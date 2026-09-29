# Role-based access control

Trellis Contracts has no HTTP API, server-route layer, or client UI in this
repository. Soroban contract entrypoints are the authoritative authorization
boundary. A future API must reject unauthorized mutations by applying the same
server-side policy; hiding a UI control is never an authorization check.

## Roles and capabilities

`shared::auth::Permission` is the central capability vocabulary.
`role_for_permission`, `has_permission`, and `require_permission` map each
capability to the required on-chain role; `require_permission` also verifies
the caller's Soroban authorization.

| Actor / role | Capabilities | Enforcement |
|---|---|---|
| End user (`EndUser`) | Use only their own user-scoped resources | Entry point checks the resource owner and requires that address to authorize |
| Maintainer (`Admin`) | Change configuration, manage roles, treasury operations, pause/resume, referral configuration, upgrades, and read maintainer audit records | Admin role (or the legacy stored admin address) plus caller authorization |
| Treasury manager (`TreasuryManager`) | Deposit and withdraw treasury funds | `TreasuryOperations` permission |
| Pauser (`Pauser`) | Pause or resume the aid contract | `PauseContracts` permission; Admin can grant or revoke this role |
| Referral manager (`ReferralManager`) | Change referral configuration | `ReferralConfiguration` permission; Admin can grant or revoke this role |
| Oracle signer (`OracleSigner`) | Submit oracle updates | Oracle's explicit registered-submitter allowlist and signature check |
| Upgrader (`Upgrader`) | Propose or execute contract upgrades | `UpgradeContracts` permission |
| Service actor (`ServiceActor`) | Call explicitly registered contract-to-contract service operations | `ServiceOperation` permission and Soroban contract authorization |

End users are authorized by ownership and signature, not by a global role that
would grant access to other users' records. A contract may use `EndUser` for an
additional allowlist policy, but must still check resource ownership.

## Permission matrix

Every privileged access-control entry point maps to one row of the matrix in
`contracts/access-control/src/permissions.rs`. `policy_for` is the
single-source-of-truth table; `require_action` is the only guard an entry point
uses, so a new privileged action adds a row rather than a new bespoke check.

| Action | Capability (`shared::auth::Permission`) | Scope | Entry points |
|---|---|---|---|
| `ManageMaintainers` | `ManageRoles` | Global | `add_admin`, `remove_admin` |
| `ConfigureRoleRegistry` | `ManageConfiguration` | Global | `create_role`, `set_role_parent` |
| `AssignRoles` | `ManageRoles` | Global | `grant_role`, `revoke_role` |
| `InviteMember(role)` | `ManageRoles` | Role-scoped to `role` | `create_invitation` |
| `CancelInvitation(inviter)` | `ManageRoles` | Owner-scoped to `inviter` | `revoke_invitation` |
| `ReadAuditTrail` | `ReadAuditTrail` | Global | `audit_trail` |

## Oracle contract authorization matrix

The oracle contract does not use the shared capability model. Its privileged
surface is governed by a stored admin address and an explicit submitter
registry. The matrix below is the authoritative reference for the oracle's
privileged entrypoints and is covered by the authorization matrix tests in
`contracts/oracle-contract/src/tests.rs`.

| Entrypoint | Required role | Allowed actor | Denied actors |
|---|---|---|---|
| `initialize(admin)` | One-time admin signature | Fresh contract + admin signature | Any caller on an already-initialized contract; any caller without the admin signature |
| `register_submitter(admin, submitter)` | Admin | Stored admin | Non-admin actors |
| `deactivate_submitter(admin, submitter)` | Admin | Stored admin | Non-admin actors |
| `set_staleness_window(admin, window)` | Admin | Stored admin | Non-admin actors |
| `submit_price(submitter, ...)` | Registered + active submitter | Active submitter with valid nonce | Unregistered actors; revoked (deactivated) submitters |

The oracle admin is not implied to be a submitter. Admin must explicitly
register an address before it can submit prices. Deactivation is the
revocation path: a deactivated submitter immediately loses the ability to
submit prices and cannot regain access without a fresh admin registration.

Scope narrows authority beyond the role:

- `**Global**` actions require maintainer authority: the contract's admin
  registry, or the shared capability model (which also honours the
  stored-admin fallback).
- `**Role-scoped**` actions are satisfied by a maintainer, or by a holder of the
  named role. That is why a role's members can invite into their own role but
  cannot invite into a role they do not hold.
- `**Owner-scoped**` actions are satisfied by a maintainer, or by the address
  that owns the record (the original inviter). A peer holding the same role
  cannot cancel someone else's invitation.

A caller outside a global action's authority is rejected with
`AccessControlError::NotAdmin`; a caller outside a role- or owner-scoped
action's breadth is rejected with
`AccessControlError::RoleEscalation`.

Admin is the maintainer super-role; the specialized manager roles provide
delegation for non-admin accounts. Admin does not imply `ServiceActor` or
`OracleSigner`, which remain explicitly registered. The stored-admin fallback
preserves maintainer permissions on deployments created before shared role
entries existed.

The treasury's configured referral contract receives `ServiceActor` when set;
replacing it revokes the prior service role. Other contract-to-contract
integrations should follow the same explicit-registration pattern.

The aid contract grants `Pauser` access through `set_pauser`. The referral
contract grants `ReferralManager` access through `set_referral_manager`.
Oracle submitters remain controlled by the oracle's existing explicit
registration/deactivation API.

## Initialization and upgrades

Shared-admin contracts use `initialize_admin`, which requires the initial
administrator's signature, records the admin role, and rejects a second
initialization. Contracts with a separate role registry apply equivalent
one-time and signature checks in their initializer. New role variants are
appended to the existing enum so stored role discriminants remain stable.

After upgrading an existing treasury deployment, its administrator must call
`set_referral_contract` with the currently configured referral address once to
grant that address the new `ServiceActor` role. No user funds or contract data
are migrated.

## Validation

```bash
cargo test -p shared auth
cargo test -p access-control
cargo test -p treasury-contract
cargo test -p oracle-contract
```

The shared auth tests cover every permission-to-role mapping and denial after
revocation. The access-control tests cover the permission matrix itself: each
action's capability and scope must match its row, a maintainer satisfies every
action, an outsider is denied every global action, and role- and owner-scoped
actions stop at the edge of their scope. Treasury contract tests cover
authorized service payouts and reject an unregistered contract actor. The
oracle contract tests cover the oracle authorization matrix above: one allow
and one deny case for each privileged entrypoint, including revoked and
unregistered actors.
