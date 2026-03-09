## Partial refunds for mid-period downgrades and cancellations

The subscription vault supports controlled partial refunds so that merchants and operators can
return a portion of a subscriber’s prepaid balance when plans are downgraded or cancelled
mid-period, without compromising balance integrity.

### Design goals

- **Safety first** – No fund creation or loss; all refunds are debits from existing balances.
- **Explicit authorization** – Only the contract admin can authorize partial refunds.
- **Predictable semantics** – Refunds operate on remaining prepaid balances and do not
  retroactively alter past charges.
- **Clear observability** – Each refund emits a dedicated event for off-chain reconciliation.

### Entry point

`partial_refund(admin, subscription_id, subscriber, amount) -> Result<(), Error>`

- **Authorization**
  - `admin` must be the contract admin; call is gated via `require_admin_auth`.
  - `subscriber` must match the subscription’s `subscriber` field.
- **Preconditions**
  - `amount > 0`
  - `amount <= subscription.prepaid_balance`
- **Effects**
  - Decrease `subscription.prepaid_balance` by `amount`.
  - Transfer `amount` of tokens from the vault contract back to `subscriber`.
  - Emit a `PartialRefundEvent { subscription_id, subscriber, amount }`.

If any precondition fails, the function returns an appropriate error
(`InvalidAmount`, `InsufficientBalance`, `Unauthorized`, etc.) and no state or
token balances are changed.

### Refund semantics

Partial refunds work against the **remaining prepaid balance**:

- Funds that have not yet been charged (unused balance) can be partially refunded.
- Previously processed charges that already credited merchant balances are not
  modified by this API; they remain part of the settlement history.

This design aligns well with common downgrade and cancellation flows:

- On cancellation, subscriptions can:
  - Use `partial_refund` to return a portion of the remaining prepaid balance, then
  - Use `withdraw_subscriber_funds` or other off-chain processes as needed.
- On mid-period downgrade, merchants can:
  - Adjust plan parameters for future periods.
  - Use `partial_refund` to return an agreed portion of the unused balance.

### Events and reconciliation

Every successful partial refund emits:

- `PartialRefundEvent` with:
  - `subscription_id`
  - `subscriber`
  - `amount`

Frontends and back-office systems can subscribe to `partial_refund` events to
reconcile on-chain token movements with off-chain ledgers, invoices, and support
workflows.

### UX and policy guidance

- **Who calls partial_refund?**
  - Typically a backend operations process or risk/finance service running as the
    contract admin.
  - Merchants can request refunds through off-chain workflows that in turn call
    this entrypoint.

- **How much to refund?**
  - Policies may base refund amounts on:
    - Time remaining in the billing period (proration).
    - Usage to date (for metered features).
    - Risk scoring or customer-service policies.
  - These policies are implemented off-chain; `partial_refund` simply enforces
    that the chosen amount is available and safe to return.

