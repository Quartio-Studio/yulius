//! Changing the resource limits (SPEC-459 HSL-004..007): a superadmin saves a number or
//! host-sized for one, removes its setting, or answers the pins that no longer fit: take the
//! host-sized values, or keep the numbers for this host.

use std::sync::Arc;

use yulius_contracts::api::updates::LimitsView;
use yulius_domain::limits::Limit;
use yulius_domain::time::Timestamp;

use super::{AUDIT_TARGET, Loaded, SavedPin, SavedSetting, amount};
use crate::auth::AuthContext;
use crate::context::App;
use crate::error::{AppError, AppResult};

impl Loaded {
    /// Saves host-sized for every pin that no longer fits; after a move (`moved`) also for the
    /// unit values seen on the other host that this unit does not hold yet. Returns what
    /// changed, in words.
    pub(super) fn adapt(&mut self, moved: bool, by: &str, at: Timestamp) -> Vec<String> {
        let mut changed = Vec::new();
        for limit in Limit::ALL {
            let stale = self.pin(limit).filter(|pin| pin.stale(limit, self.host));
            let left_behind = if moved { self.left_behind(limit) } else { None };
            let Some(pin) = stale.or(left_behind) else {
                continue;
            };
            self.record.settings.insert(
                limit.key().to_owned(),
                SavedSetting {
                    value: None,
                    host: self.host.into(),
                    by: by.to_owned(),
                    at: at.0,
                },
            );
            changed.push(format!(
                "{} {} → {}",
                limit.key(),
                amount(limit, pin.value),
                amount(limit, limit.host_sized(self.host))
            ));
        }
        changed
    }

    /// Records every pin that no longer fits for this host: the numbers stay. Returns them.
    fn keep(&mut self) -> Vec<String> {
        let mut kept = Vec::new();
        for limit in Limit::ALL {
            let Some(pin) = self.pin(limit).filter(|pin| pin.stale(limit, self.host)) else {
                continue;
            };
            let host = self.host.into();
            match self.record.settings.get_mut(limit.key()) {
                Some(saved) => saved.host = host,
                None => {
                    self.record.unit.insert(
                        limit.key().to_owned(),
                        SavedPin {
                            value: pin.value,
                            host,
                        },
                    );
                }
            }
            kept.push(format!("{} {}", limit.key(), amount(limit, pin.value)));
        }
        kept
    }
}

/// What a change did, for the audit log: nothing when it changed nothing.
struct Done {
    action: &'static str,
    target: &'static str,
    detail: String,
}

fn limit_of(key: &str) -> AppResult<Limit> {
    Limit::from_key(key).ok_or(AppError::NotFound)
}

impl App {
    /// `GET /api/admin/limits` (superadmin): every limit with its value, source and formula.
    ///
    /// # Errors
    /// `Forbidden` for others.
    pub async fn limits_view(&self, ctx: &AuthContext) -> AppResult<LimitsView> {
        self.require_superadmin(ctx)?;
        self.limits_snapshot().await
    }

    /// `PUT /api/admin/limits/{key}` (superadmin): pins `key` to `value`, or with `None` makes
    /// it host-sized whatever the unit says. Applies at once where the limit is live.
    ///
    /// # Errors
    /// `Forbidden` for others, `NotFound` for an unknown key, `Invalid` outside the range.
    pub async fn set_limit(
        self: &Arc<Self>,
        ctx: &AuthContext,
        key: &str,
        value: Option<u64>,
    ) -> AppResult<LimitsView> {
        let limit = limit_of(key)?;
        self.change_limits(ctx, |loaded, by, at| {
            if let Some(value) = value {
                limit
                    .check(value, loaded.host)
                    .map_err(|err| AppError::Invalid(err.to_string()))?;
            }
            loaded.record.settings.insert(
                limit.key().to_owned(),
                SavedSetting {
                    value,
                    host: loaded.host.into(),
                    by: by.to_owned(),
                    at: at.0,
                },
            );
            Ok(Some(Done {
                action: "limits.set",
                target: limit.key(),
                detail: value.map_or_else(|| "host-sized".to_owned(), |v| amount(limit, v)),
            }))
        })
        .await
    }

    /// `DELETE /api/admin/limits/{key}` (superadmin): forgets the setting; the unit's value is
    /// in force again, else the host-sized one.
    ///
    /// # Errors
    /// `Forbidden` for others, `NotFound` for an unknown key.
    pub async fn remove_limit(
        self: &Arc<Self>,
        ctx: &AuthContext,
        key: &str,
    ) -> AppResult<LimitsView> {
        let limit = limit_of(key)?;
        self.change_limits(ctx, |loaded, _, _| {
            Ok(loaded.record.settings.remove(limit.key()).map(|was| Done {
                action: "limits.removed",
                target: limit.key(),
                detail: was
                    .value
                    .map_or_else(|| "was host-sized".to_owned(), |v| format!("was {v}")),
            }))
        })
        .await
    }

    /// `POST /api/admin/limits/adapt` (superadmin): host-sized for every pin that no longer
    /// fits this host.
    ///
    /// # Errors
    /// `Forbidden` for others.
    pub async fn adapt_limits(self: &Arc<Self>, ctx: &AuthContext) -> AppResult<LimitsView> {
        self.change_limits(ctx, |loaded, by, at| {
            let changed = loaded.adapt(false, by, at);
            Ok((!changed.is_empty()).then(|| Done {
                action: "limits.adapted",
                target: AUDIT_TARGET,
                detail: changed.join(", "),
            }))
        })
        .await
    }

    /// `POST /api/admin/limits/keep` (superadmin): the pins that no longer fit stay and are
    /// recorded for this host.
    ///
    /// # Errors
    /// `Forbidden` for others.
    pub async fn keep_limits(self: &Arc<Self>, ctx: &AuthContext) -> AppResult<LimitsView> {
        self.change_limits(ctx, |loaded, _, _| {
            let kept = loaded.keep();
            Ok((!kept.is_empty()).then(|| Done {
                action: "limits.kept",
                target: AUDIT_TARGET,
                detail: kept.join(", "),
            }))
        })
        .await
    }

    /// Applies `change` to a copy of the limits, saves the record, puts the values in force
    /// and audits what was done.
    async fn change_limits(
        self: &Arc<Self>,
        ctx: &AuthContext,
        change: impl FnOnce(&mut Loaded, &str, Timestamp) -> AppResult<Option<Done>> + Send,
    ) -> AppResult<LimitsView> {
        self.require_superadmin(ctx)?;
        let login = self.login_of(&ctx.user.id).await;
        let now = self.now();
        let mut guard = self.limits.loaded.lock().await;
        let Some(current) = guard.as_mut() else {
            return Err(AppError::Conflict(
                "this server did not load its resource limits".into(),
            ));
        };
        let mut next = current.clone();
        let Some(done) = change(&mut next, &login, now)? else {
            return Ok(current.view());
        };
        self.save_limits(&next.record).await?;
        *current = next;
        let raised = self.put_in_force(current);
        let view = current.view();
        drop(guard);
        self.deps
            .store
            .audit(
                now,
                &login,
                done.action,
                Some(done.target),
                Some(&done.detail),
            )
            .await?;
        if raised.sessions {
            self.drain_queue().await;
        }
        if raised.lanes {
            self.ci_dispatch().await;
        }
        Ok(view)
    }
}
