# Admin Authorization Matrix

This document defines the expected authorization behavior for admin-only entrypoints in
`contracts/subscription_vault/src/lib.rs`.

## Guard semantics

Admin-only entrypoints that accept an `admin` or `current_admin` address use a single
guard:

1. The supplied address must satisfy Soroban `require_auth()`.
2. The supplied address must equal the admin address currently stored in contract state.

If signature verification fails, Soroban aborts with an auth-host error.
If signature verification succeeds but the supplied address is not the stored admin, the
contract returns `Error::Unauthorized` (`401`).

`Error::Forbidden` (`403`) is reserved for non-admin actor/resource mismatches elsewhere
in the contract, such as subscriber or merchant operations authorized by the wrong actor.

## Matrix

| Entrypoint | Current admin | Stale admin after rotation | Other non-admin signer | Error on mismatch |
|-----------|---------------|----------------------------|------------------------|-------------------|
| `set_min_topup` | Allowed | Denied | Denied | `Unauthorized` |
| `rotate_admin` | Allowed | Denied | Denied | `Unauthorized` |
| `recover_stranded_funds` | Allowed | Denied | Denied | `Unauthorized` |
| `add_accepted_token` | Allowed | Denied | Denied | `Unauthorized` |
| `remove_accepted_token` | Allowed | Denied | Denied | `Unauthorized` |
| `enable_emergency_stop` | Allowed | Denied | Denied | `Unauthorized` |
| `disable_emergency_stop` | Allowed | Denied | Denied | `Unauthorized` |
| `export_contract_snapshot` | Allowed | Denied | Denied | `Unauthorized` |
| `export_subscription_summary` | Allowed | Denied | Denied | `Unauthorized` |
| `export_subscription_summaries` | Allowed | Denied | Denied | `Unauthorized` |
| `set_billing_retention` | Allowed | Denied | Denied | `Unauthorized` |
| `compact_billing_statements` | Allowed | Denied | Denied | `Unauthorized` |
| `set_oracle_config` | Allowed | Denied | Denied | `Unauthorized` |
| `set_subscriber_credit_limit` | Allowed | Denied | Denied | `Unauthorized` |

## Rotation and stale-auth notes

- Admin rotation takes effect immediately in the same transaction that updates storage.
- Any previously valid admin address becomes stale as soon as rotation completes.
- Reusing a stale admin address in a later transaction is rejected deterministically with
  `Error::Unauthorized`.
- Export hooks remain read-only, but still require the current stored admin.

## Test coverage

The negative authorization matrix is covered in:

- `test_admin_authorization_matrix_rejects_non_admin_across_protected_entrypoints`
- `test_admin_authorization_matrix_rejects_stale_admin_after_rotation`

Additional rotation-focused regression tests remain in
`contracts/subscription_vault/src/test.rs` and
`contracts/subscription_vault/src/test_security.rs`.
