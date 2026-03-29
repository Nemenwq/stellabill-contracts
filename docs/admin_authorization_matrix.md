# Admin Authorization Matrix

This document defines the expected authorization behavior for every admin-only
entrypoint in `subscription_vault`.

## Error semantics

- `Unauthorized` (`401`): the caller signed the transaction, but the signed
  address is not the stored admin for an admin-only route.
- `Forbidden` (`403`): the caller is authenticated, but the route is governed
  by a different role model entirely, such as subscriber-or-merchant ownership.
- Host auth failure (`Error(Auth, InvalidAction)` in tests): no valid signature
  was supplied for a required `require_auth()` check.

Admin-only routes should not return `Forbidden`; they either succeed for the
stored admin, return `Unauthorized` for a stale or non-admin signer, or fail at
the host layer when no signature is present.

## Matrix

| Entrypoint | Authorization model | Non-admin result | Stale admin after rotation | Notes |
| --- | --- | --- | --- | --- |
| `set_min_topup` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Centralized via `require_admin_auth` |
| `rotate_admin` | Current admin must match stored admin | `Unauthorized` | `Unauthorized` | New admin takes effect immediately |
| `recover_stranded_funds` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Recovery reason still validated separately |
| `add_accepted_token` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Token config only |
| `remove_accepted_token` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Default token removal still rejected as `InvalidInput` |
| `enable_emergency_stop` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Idempotent when already enabled |
| `disable_emergency_stop` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Idempotent when already disabled |
| `export_contract_snapshot` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Export-only surface |
| `export_subscription_summary` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Single-subscription export |
| `export_subscription_summaries` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Paged export |
| `set_subscriber_credit_limit` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Subscriber risk control |
| `partial_refund` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Subscriber parameter mismatch is also `Unauthorized` |
| `set_billing_retention` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Statement retention policy |
| `compact_billing_statements` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Maintenance operation |
| `set_oracle_config` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Config validation happens after auth |
| `remove_from_blocklist` | Explicit `admin` arg must equal stored admin | `Unauthorized` | `Unauthorized` | Uses the same centralized guard |
| `batch_charge` | Stored admin is loaded from state and must sign | Host auth failure if unsigned | New admin signature required after rotation | No caller-supplied admin parameter |

## Rotation and replay notes

- Admin rotation is atomic: once `rotate_admin` succeeds, the old admin loses
  access to all admin-only routes in the same state version.
- Reusing an old auth context after rotation must fail as `Unauthorized` for
  explicit-admin routes.
- `batch_charge` is the one exception to the explicit-admin pattern because it
  reads the stored admin internally; after rotation, only the new stored admin's
  signature satisfies the host auth check.
