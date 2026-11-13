-- Yulius migration 0055 (SPEC-483 V6, ADR-0324): a production secret has no hosts. Since
-- SPEC-134 a secret allowed for production deploys cannot be bound to a host, but a row made
-- one before that kept its hosts, and the egress proxy still sent it there for an agent's run
-- while every other use refused it. The rule is one now (no run but a production deploy gets
-- it), so the hosts such a row kept say nothing: they are removed, and the audit log says
-- which they were (schema version 55).
INSERT INTO audit_log (at, actor, action, target, detail)
    SELECT CAST(strftime('%s', 'now') AS INTEGER) * 1000,
           'yulius',
           'vault.agent.hosts_cleared',
           id,
           json_object('name', name, 'sandbox', sandbox_id, 'hosts', json(hosts),
                       'reason', 'a production secret is sent to no host')
    FROM vault_secrets
    WHERE cd_prod = 1 AND hosts != '[]';

UPDATE vault_secrets SET hosts = '[]' WHERE cd_prod = 1 AND hosts != '[]';
