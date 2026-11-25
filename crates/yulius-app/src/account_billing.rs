//! When each subscription charges its card (SPEC-651, ADR-0478). No provider tells Yulius the
//! billing date, so an administrator gives one charge date and how often it repeats; every view
//! of the subscription carries the next charge, counted from today in UTC.

use std::collections::HashMap;

use yulius_contracts::api::admin::{AccountBillingView, AccountView, SetAccountBilling};
use yulius_domain::billing::BillingCycle;
use yulius_domain::cron::CivilDate;
use yulius_domain::ids::AccountId;
use yulius_domain::time::Timestamp;

use crate::accounts::{account_view, actor};
use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};
use crate::model::{AccountRecord, AttemptRecord};

/// The view of a cycle on `today`.
fn billing_view(cycle: BillingCycle, today: CivilDate) -> AccountBillingView {
    AccountBillingView {
        anchor: cycle.anchor().to_string(),
        period: cycle.period().as_str().to_owned(),
        next_charge_on: cycle.next_charge(today).to_string(),
    }
}

/// Today's date in UTC.
fn today(now: Timestamp) -> AppResult<CivilDate> {
    CivilDate::from_timestamp(now, 0)
        .ok_or_else(|| AppError::Internal("the clock is outside the calendar".into()))
}

impl App {
    /// The billing view of every subscription that has a cycle, by id, counted from now.
    pub(crate) async fn billing_views(&self) -> AppResult<HashMap<AccountId, AccountBillingView>> {
        let today = today(self.now())?;
        Ok(self
            .deps
            .store
            .account_billing()
            .await?
            .into_iter()
            .map(|(id, cycle)| (id, billing_view(cycle, today)))
            .collect())
    }

    /// [`account_view`] with the subscription's billing cycle (BIL-004): what the server answers
    /// and publishes about one subscription.
    pub(crate) async fn account_view_with_billing(
        &self,
        account: &AccountRecord,
        live: &[AttemptRecord],
    ) -> AppResult<AccountView> {
        let mut view = account_view(account, live);
        view.billing = self.billing_views().await?.remove(&account.id);
        Ok(view)
    }

    /// Sets when a subscription charges its card (BIL-003): a charge date and its period,
    /// checked before anything is written; audited `account.billing_set`.
    ///
    /// # Errors
    /// `Forbidden` for non-administrators, `NotFound`, `Invalid` for a refused date or period.
    pub async fn set_account_billing(
        &self,
        ctx: &AuthContext,
        id: &str,
        req: &SetAccountBilling,
    ) -> AppResult<AccountView> {
        self.require_instance_admin(ctx)?;
        let account = self.load_account(id).await?;
        let cycle = BillingCycle::parse(&req.anchor, &req.period)
            .map_err(|e| AppError::Invalid(e.to_string()))?;
        let detail = format!("anchor={} period={}", cycle.anchor(), cycle.period());
        self.change_billing(
            ctx,
            &account,
            Some(cycle),
            "account.billing_set",
            Some(&detail),
        )
        .await
    }

    /// Removes a subscription's charge date (BIL-003), also when it has none; audited
    /// `account.billing_removed` with the cycle removed.
    ///
    /// # Errors
    /// `Forbidden` for non-administrators, `NotFound`.
    pub async fn remove_account_billing(
        &self,
        ctx: &AuthContext,
        id: &str,
    ) -> AppResult<AccountView> {
        self.require_instance_admin(ctx)?;
        let account = self.load_account(id).await?;
        // The audit says what was removed.
        let was = self
            .deps
            .store
            .account_billing()
            .await?
            .into_iter()
            .find(|(of, _)| *of == account.id)
            .map_or_else(
                || "none was set".to_owned(),
                |(_, cycle)| format!("was anchor={} period={}", cycle.anchor(), cycle.period()),
            );
        self.change_billing(ctx, &account, None, "account.billing_removed", Some(&was))
            .await
    }

    async fn change_billing(
        &self,
        ctx: &AuthContext,
        account: &AccountRecord,
        cycle: Option<BillingCycle>,
        action: &str,
        detail: Option<&str>,
    ) -> AppResult<AccountView> {
        let now = self.now();
        let store = &self.deps.store;
        store.set_account_billing(&account.id, cycle, now).await?;
        store
            .audit(now, &actor(ctx), action, Some(account.id.as_str()), detail)
            .await?;
        // Read again, so the view says what the subscription is now.
        let account = self.load_account(account.id.as_str()).await?;
        let view = self
            .account_view_with_billing(&account, &self.working_attempts().await?)
            .await?;
        self.publish_account(view.clone());
        Ok(view)
    }
}
