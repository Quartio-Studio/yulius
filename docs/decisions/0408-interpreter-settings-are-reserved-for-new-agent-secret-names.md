# 0408. Interpreter settings are reserved for new agent-secret names

## Status

Accepted for SPEC-574, the compatible portion of issue #638.

## Context

Agents receive vault placeholders as environment variables. Interpreters read some variable names as settings even when the value is a placeholder. The vault also validates names when listing secrets and preparing their run environments. GitHub's CLI accepts credentials through two specific environment variables. [SPEC-004 GH-003](../../.ai/specs/SPEC-004-gh-cli-login.md)

## Decision

Reserve the exact names `PYTHONPATH`, `PERL5LIB`, `RUBYOPT`, `JAVA_TOOL_OPTIONS` and `JDK_JAVA_OPTIONS` when assigning an agent-secret name, including renames and approvals that create copies. `NODE_OPTIONS` already falls under the reserved `NODE_` prefix. `_JAVA_OPTIONS` already fails the initial-letter requirement.

Existing secrets with one of the five newly reserved names keep their scope, hosts, value and placeholder. The Secrets page tells each manager that the placeholder sets an interpreter option and suggests an allowed replacement name. The manager decides when to change the name and update its consumers.

Keep `GH_TOKEN` and `GITHUB_TOKEN` available for host-bound credentials. The gh shim honors a user-supplied token and the egress proxy substitutes the placeholder on the secret's bound hosts. Scope precedence and access follow [ADR-0186](0186-agent-secrets-are-project-global-or-personal.md).

## Alternatives

Applying reservations to stored names would lose access on upgrade. Automatically renaming them would break existing consumers. Restricting only agent-originated creates would leave other creation paths able to set the same interpreter options. Reserving the two GitHub token names would remove a supported credential override.

## Consequences

A manager can rotate an old value under the same name, or rename it and update callers. Until the rename, the old name can still affect interpreters in runs that receive its placeholder. An outstanding production-copy request for a newly reserved name can be dismissed and requested again with a different name. The remaining GitHub reservation in #638 requires the owner's decision about GH-003.
