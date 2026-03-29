# Subscriber Blocklist

## Overview

The subscriber blocklist mechanism allows admins and merchants to prevent specific subscriber addresses from creating new subscriptions or using subscriber-controlled mutation flows that would extend, restore, or reconfigure access. This feature is designed for fraud prevention, dispute management, and access control while preserving existing financial obligations, balances, and an auditable admin-only unblock path.

## Key Principles

1. **Preventive, Not Punitive**: The blocklist prevents blocked subscribers from creating or re-activating access but does not seize or move existing funds.
2. **Existing Obligations Preserved**: Blocklisted subscribers retain access to their existing subscriptions and prepaid balances.
3. **Dual Authorization**: Both admins (global) and merchants (scoped) can add subscribers to the blocklist.
4. **Admin-Only Removal**: Only admins can remove subscribers from the blocklist.
5. **Stable Audit Trail**: Re-adding an already blocked subscriber is rejected so the original entry and reason are not overwritten.

## Authorization Model

### Admin Authorization
- **Scope**: Global - can blocklist any subscriber address
- **Operations**: Add to blocklist, remove from blocklist
- **Use Cases**: Platform-wide fraud prevention, regulatory compliance, terms of service violations

### Merchant Authorization
- **Scope**: Limited - can only blocklist subscribers with whom they have an active subscription relationship
- **Operations**: Add to blocklist only (cannot remove)
- **Use Cases**: Payment disputes, chargebacks, merchant-specific fraud prevention

## Blocklist Enforcement

### Blocked Operations

When a subscriber is blocklisted, the following operations are **blocked**:

1. **`create_subscription`**: Cannot create new subscriptions
2. **`create_subscription_with_token`**: Cannot create token-specific subscriptions
3. **`create_subscription_from_plan`**: Cannot create subscriptions from plan templates
4. **`deposit_funds`**: Cannot deposit additional funds into existing subscriptions
5. **`resume_subscription`**: Cannot resume when the blocked subscriber is the authorizer
6. **`migrate_subscription_to_plan`**: Cannot migrate an existing subscription to a new plan version

All blocked operations return `Error::SubscriberBlocklisted`.

### Allowed Operations

Blocklisted subscribers can still perform the following operations on **existing** subscriptions:

1. **`cancel_subscription`**: Can cancel their subscriptions
2. **`pause_subscription`**: Can pause their subscriptions
3. **`withdraw_subscriber_funds`**: Can withdraw remaining balance after cancellation
4. **Charging**: Existing subscriptions continue to be charged normally (admin/automated charges)

Merchant- or admin-authorized maintenance flows remain subject to their normal authorization rules. The blocklist specifically rejects blocked subscriber-authored mutation paths.

## Storage Schema

### Blocklist Entry

```rust
pub struct BlocklistEntry {
    pub subscriber: Address,
    pub added_by: Address,
    pub added_at: u64,
    pub reason: Option<String>,
}
```

### Storage Key

Blocklist entries are stored with the key pattern:
```
(Symbol("blocklist"), subscriber_address) -> BlocklistEntry
```

This allows O(1) lookup during subscription creation, deposit, resume, and migration checks.

## Events

### BlocklistAddedEvent

Emitted when a subscriber is added to the blocklist.

```rust
pub struct BlocklistAddedEvent {
    pub subscriber: Address,
    pub added_by: Address,
    pub timestamp: u64,
    pub reason: Option<String>,
}
```

**Topic**: `("blocklist_added",)`

### BlocklistRemovedEvent

Emitted when a subscriber is removed from the blocklist.

```rust
pub struct BlocklistRemovedEvent {
    pub subscriber: Address,
    pub removed_by: Address,
    pub timestamp: u64,
}
```

**Topic**: `("blocklist_removed",)`

## API Reference

### `add_to_blocklist`

Add a subscriber to the blocklist.

```rust
pub fn add_to_blocklist(
    env: Env,
    authorizer: Address,
    subscriber: Address,
    reason: Option<String>,
) -> Result<(), Error>
```

**Authorization**:
- Admin: Can blocklist any subscriber
- Merchant: Can only blocklist subscribers they have subscriptions with

**Errors**:
- `Error::Forbidden`: Merchant attempting to blocklist unrelated subscriber
- `Error::Unauthorized`: Invalid authorization
- `Error::InvalidInput`: Subscriber is already blocklisted

### `remove_from_blocklist`

Remove a subscriber from the blocklist. Admin only.

```rust
pub fn remove_from_blocklist(
    env: Env,
    admin: Address,
    subscriber: Address,
) -> Result<(), Error>
```

**Authorization**: Admin only

**Errors**:
- `Error::Unauthorized`: Caller is not admin
- `Error::NotFound`: Subscriber is not blocklisted

### `is_blocklisted`

Check if a subscriber is blocklisted.

```rust
pub fn is_blocklisted(
    env: Env,
    subscriber: Address,
) -> bool
```

**Returns**: `true` if subscriber is blocklisted, `false` otherwise

### `get_blocklist_entry`

Get blocklist entry details for a subscriber.

```rust
pub fn get_blocklist_entry(
    env: Env,
    subscriber: Address,
) -> Result<BlocklistEntry, Error>
```

**Errors**:
- `Error::NotFound`: Subscriber is not blocklisted

## Use Cases

### 1. Fraud Prevention (Admin)

An admin detects fraudulent activity from a subscriber address:

```rust
client.add_to_blocklist(
    &admin,
    &fraudulent_subscriber,
    &Some(String::from_str(&env, "Fraudulent chargebacks detected"))
);

let result = client.try_create_subscription(
    &fraudulent_subscriber,
    &merchant,
    &amount,
    &interval,
    &false,
    &None
);
assert_eq!(result, Err(Ok(Error::SubscriberBlocklisted)));
```

### 2. Payment Disputes (Merchant)

A merchant experiences repeated payment disputes with a subscriber:

```rust
client.add_to_blocklist(
    &merchant,
    &subscriber,
    &Some(String::from_str(&env, "Repeated payment disputes"))
);

let result = client.try_deposit_funds(&sub_id, &subscriber, &amount);
assert_eq!(result, Err(Ok(Error::SubscriberBlocklisted)));
```

### 3. Regulatory Compliance (Admin)

An admin needs to restrict access for regulatory reasons:

```rust
client.add_to_blocklist(
    &admin,
    &restricted_subscriber,
    &Some(String::from_str(&env, "Regulatory compliance - sanctioned address"))
);

client.remove_from_blocklist(&admin, &restricted_subscriber);

let new_sub_id = client.create_subscription(
    &restricted_subscriber,
    &merchant,
    &amount,
    &interval,
    &false,
    &None
);
```

## Edge Cases and Limitations

### 1. Existing Subscriptions

**Behavior**: Blocklisting does not cancel or confiscate existing subscriptions. Existing subscriptions continue to function for charging and unwind flows, but blocked subscribers cannot use subscriber-authored mutation paths that would create, restore, or reconfigure access.

**Rationale**: Preserves financial obligations and prevents unilateral fund seizure while still cutting off subscriber-controlled access expansion.

### 2. Multiple Merchants

**Behavior**: If a subscriber is blocklisted by one merchant, they cannot create subscriptions with any merchant.

**Rationale**: The blocklist is global at the contract level. Merchant authorization is for adding entries, not for scoping enforcement.

### 3. Deposit Restrictions

**Behavior**: Blocklisted subscribers cannot deposit funds into existing subscriptions.

**Rationale**: Prevents blocklisted users from extending their access through top-ups.

### 4. Resume and Migration Restrictions

**Behavior**: Blocklisted subscribers cannot resume a paused or recoverable subscription and cannot migrate a subscription to a newer plan version until an admin removes the blocklist entry.

**Rationale**: Resuming or migrating materially changes subscription access and commercial terms, so these flows are treated the same as creation and top-up paths.

### 5. Charging Continues

**Behavior**: Existing subscriptions continue to be charged normally even after blocklisting.

**Rationale**: Honors existing financial commitments. If a merchant wants to stop charging, they should cancel the subscription.

### 6. Withdrawal Rights

**Behavior**: Blocklisted subscribers can withdraw remaining balance after cancellation.

**Rationale**: Prevents fund seizure. The blocklist is preventive, not punitive.

## Security Considerations

### 1. Authorization Checks

- All blocklist operations require proper authorization (admin or merchant)
- Merchant authorization is scoped to subscribers they have relationships with
- Removal is admin-only to prevent unauthorized unblocking

### 2. Storage Efficiency

- Blocklist uses O(1) lookup via direct address key
- No iteration required during subscription creation, deposit, resume, or migration
- Minimal gas overhead for blocklist checks

### 3. Event Transparency

- All blocklist additions and removals emit events
- Events include reason metadata for audit trails
- Off-chain systems can monitor blocklist changes

### 4. Duplicate Entry Protection

- Adding an already blocklisted subscriber is rejected with `Error::InvalidInput`
- The original `BlocklistEntry` remains unchanged, preserving `added_by`, `added_at`, and `reason`
- This prevents accidental audit-trail rewrites before an explicit admin unblock

### 5. No Retroactive Enforcement

- Blocklist does not cancel or modify existing subscriptions
- Prevents unexpected state changes for active subscriptions
- Maintains contract predictability

## Testing Coverage

The blocklist implementation includes tests covering:

- Admin add and admin-only removal
- Duplicate add rejection without entry mutation
- Missing-entry unblock rejection
- Add/remove event emission and payload verification
- `None` and empty-string reason variants
- Enforcement on direct creation, token-specific creation, plan creation, deposit, resume, and plan migration
- Post-unblock restoration of create, deposit, resume, and migration flows

## Operational Guidance

### For Admins

1. Document blocklist reasons whenever possible.
2. Review entries periodically and remove them only after policy review.
3. Monitor add/remove events to keep an audit trail.

### For Merchants

1. Use the blocklist as a last resort after dispute-resolution steps.
2. Escalate serious fraud cases to the admin for removal governance and platform-wide tracking.
3. Remember that merchant-added entries are enforced globally by this contract.

### For Subscribers

1. Appeal blocklist status through the contract admin.
2. Existing balances are not confiscated, but new subscriber-driven mutations stay blocked until removal.

## Future Enhancements

Potential future improvements:

1. Merchant-scoped enforcement instead of global enforcement
2. Temporary blocklist entries with expiry
3. Categorized blocklist reasons for differentiated policy
4. Batch add/remove operations
5. Export tooling for compliance reporting
