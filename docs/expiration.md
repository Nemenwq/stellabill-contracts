# Expiration Rules and Cleanup Semantics

This document outlines the expiration lifecycle, cleanup mechanisms, and fund safety guarantees for subscriptions in the StellaBill system.

## 1. Expiration Model

Subscriptions include two time-based properties to manage their active duration:
- `start_time` (`u64`): The timestamp when the subscription was created.
- `expires_at` (`Option<u64>`): The timestamp after which the subscription is considered expired. If `None`, the subscription runs indefinitely (unless manually cancelled or lifetime cap is reached).

A subscription is considered expired when:
```rust
current_time >= expires_at
```

## 2. State Transitions

Subscriptions transition through several states, but expiration introduces explicit guards:

- **Active**: The subscription is actively charging and valid.
- **Expired**: Evaluated dynamically based on `is_expired()`. This state takes precedence over active billing operations.
- **Cancelled**: A terminal state explicitly triggered by the user or system.
- **Archived**: A clean-up state that preserves essential data and allows fund withdrawals while preventing all other operations.

**Important Distinction:**
- **Expired** is time-based and automatic. A subscription whose `expires_at` is reached is immediately ineligible for charging, even if its state is nominally `Active`.
- **Cancelled** is user-driven or system-driven (e.g., reaching a lifetime cap).
- These states are mutually exclusive in behavior. An expired subscription cannot be cancelled, but both can be **Archived**.

## 3. Expiration Effects

When a subscription is expired (`is_expired == true`):

- **Rejected Operations**:
  - New periodic charges (`charge_subscription`)
  - New usage-based charges (`charge_usage`)
  - New fund deposits (`deposit_funds`)
  - Explicit cancellation (`cancel_subscription`)

- **Allowed Operations**:
  - Subscriber fund withdrawals (`withdraw_subscriber_funds`)
  - Metadata reads and general state queries
  - Archival cleanup (`cleanup_subscription`)

## 4. Cleanup Semantics & Archival Strategy

Instead of deleting expired or cancelled subscriptions (which could corrupt the state and lead to fund loss), StellaBill uses an **Archival Strategy**.

The `cleanup_subscription` function allows moving a terminal subscription (either Cancelled or Expired) into the `Archived` state.

### Archival Guarantees:
- **No Deletion**: The subscription entity is preserved. Critical fields (balances, identities) remain intact.
- **Readability**: Archived entities can still be read by indexers and clients.
- **Safety**: Moving to `Archived` enforces strict terminal behavior, ensuring no accidental resumption or modification.

## 5. Fund Safety Guarantee

A core invariant of the StellaBill protocol is that **funds are never deleted**.
- If a subscription expires or is archived, any remaining escrowed funds in `prepaid_balance` remain assigned to that subscription.
- The `withdraw_subscriber_funds` function explicitly permits withdrawals when the status is `Expired`, `Cancelled`, or `Archived`.
- This ensures subscribers can always retrieve their unused prepaid balances, regardless of the subscription's terminal state.

## 6. Examples

### Flow 1: Expiration without Cancellation
1. Subscription created with `expires_at = T`.
2. Time passes. Current time becomes `>= T`.
3. The subscription is now automatically **Expired**. New charges fail.
4. The user or merchant calls `cleanup_subscription`.
5. State transitions to **Archived**.
6. The user withdraws their remaining funds.

### Flow 2: Cancellation before Expiration
1. Subscription created with `expires_at = T`.
2. Current time is `< T`. User calls `cancel_subscription`.
3. State explicitly transitions to **Cancelled**.
4. The user or merchant calls `cleanup_subscription`.
5. State transitions to **Archived**.
6. User withdraws funds.

## 7. Indexer Guidance

Indexers tracking the state of subscriptions should:
1. Always compute `is_expired = current_time >= expires_at` when displaying active subscriptions.
2. Treat `Archived` subscriptions as immutable, terminal records.
3. Monitor `SubscriptionExpiredEvent` and `SubscriptionArchivedEvent` to trigger backend cleanups or UI updates.
