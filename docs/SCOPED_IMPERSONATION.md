# Scoped Maintainer Impersonation for Support Debugging

## Overview

The Scoped Maintainer Impersonation feature allows authorized maintainers to temporarily impersonate users for debugging purposes while maintaining strict security boundaries. This enables safe reproduction of user-reported issues without granting broad access to private data or enabling irreversible actions.

## Features

### 1. Time-Limited Sessions
- Configurable session duration (default: 15 minutes, maximum: 1 hour)
- Automatic session expiration
- Manual session revocation capability
- Session state tracking (Active, Revoked, Expired, Completed)

### 2. Scoped Permissions
- **ReadOnly**: Read-only access to user data
- **Diagnostic**: Read access plus diagnostic operations (no state changes)
- **SafeMutations**: Read access plus safe state changes (non-critical operations)
- **FullAccess**: Full access within time limit (use with caution)

### 3. Resource-Level Scoping
- Limit impersonation to specific resource types (AID, Payment, Escrow, Treasury, Roles, Config)
- Restrict to specific resource IDs for fine-grained control
- Prevents accidental access to unrelated user data

### 4. Dangerous Mutation Blocking
- Automatically blocks dangerous operations unless explicitly allowed
- Treasury operations, role management, and critical changes require explicit permission
- Prevents accidental irreversible actions during debugging

### 5. Comprehensive Audit Logging
- All impersonation sessions are logged with full context
- Every action taken during impersonation is recorded
- Integration with existing timeline and audit trail systems
- Events emitted for real-time monitoring

### 6. Visible Indicators
- Ledger events for session creation, revocation, and actions
- `is_being_impersonated()` function to check active sessions
- Session metadata includes impersonator, target, scope, and duration
- Clear distinction between normal and impersonated operations

## Usage

### Creating an Impersonation Session

```rust
use trellis_contracts::{
    create_session, revoke_session, validate_session, check_action_permission,
    record_action, ImpersonationScope, ResourceType, ResourceScope, SessionParams,
    ImpersonationAction, SessionState,
};

// Prepare session parameters
let params = SessionParams {
    target_user: user_address,
    scope: ImpersonationScope::ReadOnly,
    resource_scopes: Vec::new(&env),
    duration_sec: 900, // 15 minutes
    allow_dangerous_mutations: false,
    reason: symbol_short!("debug_user_issue"),
};

// Create the session (requires Support role)
let result = create_session(&env, &maintainer_address, params)?;
let session = result.session;

println!("Session {} created for {}", session.session_id, session.target_user);
```

### Checking Action Permissions

```rust
// Before performing an action, check if it's permitted
let permission = check_action_permission(
    &env,
    &session,
    ImpersonationAction::Read,
    ResourceType::Aid,
    Some(aid_id),
);

match permission {
    Ok(()) => {
        // Action is permitted, proceed and record it
        record_action(
            &env,
            session.session_id,
            ImpersonationAction::Read,
            ResourceType::Aid,
            Some(aid_id),
        )?;
        // Perform the actual action...
    }
    Err(ImpersonationError::ActionNotPermitted) => {
        // Action not permitted in current scope
        return Err(Error::Unauthorized);
    }
    Err(ImpersonationError::DangerousMutationBlocked) => {
        // Dangerous action requires explicit permission
        return Err(Error::ContractPaused); // Or appropriate error
    }
    Err(e) => {
        // Other error (session expired, etc.)
        return Err(Error::from(e));
    }
}
```

### Resource-Level Scoping

```rust
// Limit impersonation to specific AID record
let mut resource_scopes = Vec::new(&env);
resource_scopes.push_back(ResourceScope {
    resource_type: ResourceType::Aid,
    resource_id: Some(123), // Only AID #123
});

let params = SessionParams {
    target_user: user_address,
    scope: ImpersonationScope::Diagnostic,
    resource_scopes,
    duration_sec: 900,
    allow_dangerous_mutations: false,
    reason: symbol_short!("debug_specific_aid"),
};
```

### Checking Active Impersonation

```rust
// Check if a user is currently being impersonated
if is_being_impersonated(&env, &user_address) {
    // Get the current session
    if let Some(session) = get_current_impersonation(&env, &user_address) {
        println!("User {} is being impersonated by {}", 
            user_address, session.impersonator);
        println!("Session scope: {:?}", session.scope);
        println!("Expires at: {}", session.expires_at);
    }
}
```

### Revoking a Session

```rust
// Revoke an active session
revoke_session(&env, &maintainer_address, session_id)?;
```

## Security Model

### Authorization Requirements

To create an impersonation session, the caller must:
- Hold the `Support` role
- Be authorized with `Permission::ImpersonateUser`

To revoke a session, the caller must:
- Be the session creator, OR
- Hold `Permission::ImpersonateUser`

### Target User Restrictions

- Admin users cannot be impersonated
- Target users must not hold sensitive roles (Admin, TreasuryManager, etc.)
- Session creation validates target user eligibility

### Scope Validation

- `ReadOnly` and `Diagnostic` scopes cannot include Treasury or Roles resources
- Resource scopes must be compatible with the chosen permission scope
- Invalid scope combinations are rejected at session creation

### Time Limits

- Maximum session duration: 1 hour (3600 seconds)
- Default session duration: 15 minutes (900 seconds)
- Sessions auto-expire when the ledger timestamp exceeds `expires_at`
- Expired sessions are automatically cleaned from active indexes

### Concurrent Session Limits

- Maximum 5 concurrent sessions per impersonator
- Prevents session abuse and helps maintain accountability
- Attempts to exceed limit return `TooManyActiveSessions` error

## Audit Trail

### Session Creation Events

```rust
// Emitted when a session is created
env.events().publish(
    (symbol_short!("impersonation"), symbol_short!("session_created")),
    (session_id, impersonator, target_user, expires_at),
);

// Logged to action audit trail
record_action_audit_event(
    env,
    impersonator,
    TimelineEventType::ConfigChanged,
    &resource_link_for_session(env, session_id),
    symbol_short!("impersonation"),
    symbol_short!("create"),
    Some(reason),
    Some(target_user),
    Some(symbol_short!("scope")),
    Some(i128::from(scope as u8)),
    Some(i128::from(expires_at)),
)?;
```

### Action Events

```rust
// Emitted for each action during impersonation
env.events().publish(
    (symbol_short!("impersonation"), symbol_short!("action_recorded")),
    (session_id, action_type.as_symbol(), resource_type.as_symbol()),
);

// Logged to action audit trail
record_action_audit_event(
    env,
    impersonator,
    TimelineEventType::RecordUpdated,
    &resource_link_for_resource(env, resource_type, resource_id),
    symbol_short!("impersonation"),
    symbol_short!("action"),
    Some(action_type.as_symbol()),
    Some(target_user),
    Some(resource_type.as_symbol()),
    resource_id.map(|id| i128::from(id)),
    Some(i128::from(action_count)),
)?;
```

### Session Revocation Events

```rust
// Emitted when a session is revoked
env.events().publish(
    (symbol_short!("impersonation"), symbol_short!("session_revoked")),
    (session_id, caller),
);
```

## Error Handling

### ImpersonationError

| Error Code | Description |
|------------|-------------|
| `SessionNotFound` | Session does not exist or has expired |
| `Unauthorized` | Caller lacks impersonation permission |
| `InvalidScope` | Requested scope is invalid |
| `DurationTooLong` | Duration exceeds maximum allowed |
| `SessionExpired` | Session has expired |
| `ActionNotPermitted` | Action not permitted in current scope |
| `DangerousMutationBlocked` | Dangerous mutation requires explicit permission |
| `SessionAlreadyActive` | Session already active (idempotency check) |
| `TargetNotImpersonatable` | Target user cannot be impersonated |
| `TooManyActiveSessions` | Maximum concurrent sessions exceeded |

## Integration with Existing Systems

### Authorization System

- Uses existing `Permission::ImpersonateUser` permission
- Integrates with `Role::Support` for authorization
- Leverages existing `has_permission()` and `require_permission()` functions
- Consistent with existing role-based access control

### Timeline and Audit Trail

- Uses existing `record_action_audit_event()` for logging
- Integrates with `TimelineEventType::ConfigChanged` and `RecordUpdated`
- Leverages existing `ResourceLink` structure for resource tracking
- Maintains consistency with existing audit patterns

### Event System

- Emits structured events using existing event patterns
- Topics follow existing naming conventions (`impersonation:*`)
- Data payloads use existing Soroban event patterns
- Compatible with existing event indexing infrastructure

## Best Practices

### For Maintainers

1. **Use the most restrictive scope possible**
   - Start with `ReadOnly` and only escalate if necessary
   - Use resource-level scoping to limit access to specific data
   - Avoid `FullAccess` unless absolutely required

2. **Keep sessions short**
   - Use the default 15-minute duration for most cases
   - Only extend to 1 hour for complex debugging scenarios
   - Revoke sessions immediately when debugging is complete

3. **Document the reason**
   - Always provide a clear reason for impersonation
   - Use specific reason codes that can be tracked
   - This improves audit trail quality

4. **Review session logs**
   - Check action logs after impersonation sessions
   - Verify that only intended actions were taken
   - Escalate any suspicious activity

### For Developers

1. **Always check permissions before actions**
   - Use `check_action_permission()` before any state change
   - Handle permission errors gracefully
   - Record actions even if they fail (for audit trail)

2. **Validate session before use**
   - Call `validate_session()` to ensure session is still active
   - Handle session expiration gracefully
   - Provide clear error messages to users

3. **Resource scoping for precision**
   - Use resource scoping when debugging specific issues
   - Prevents accidental access to unrelated data
   - Improves security and audit quality

4. **Test impersonation flows**
   - Include tests for impersonation scenarios
   - Test scope enforcement and permission checks
   - Verify audit trail completeness

## Testing

### Unit Tests

The implementation includes comprehensive tests covering:

- **Session Creation**: Basic session creation with proper authorization
- **Unauthorized Access**: Prevention of unauthorized session creation
- **Duration Limits**: Enforcement of maximum session duration
- **Session Expiration**: Automatic expiration when time limit is reached
- **Scope Enforcement**: Read-only scope blocks mutations
- **Dangerous Mutation Blocking**: Prevention of dangerous operations
- **Session Revocation**: Manual session termination
- **Resource Scoping**: Resource-level access control
- **Active Session Detection**: Checking if a user is being impersonated
- **Concurrent Session Limits**: Enforcement of maximum concurrent sessions

### Running Tests

```bash
cargo test --package shared test_impersonation
```

### Test Scenarios

#### Allowed Actions
```rust
#[test]
fn test_read_action_permitted_in_readonly_scope() {
    // Read actions should be allowed in ReadOnly scope
}
```

#### Denied Actions
```rust
#[test]
fn test_mutation_blocked_in_readonly_scope() {
    // Mutations should be blocked in ReadOnly scope
}
```

#### Expired Sessions
```rust
#[test]
fn test_expired_session_rejected() {
    // Expired sessions should be rejected
}
```

#### Audited Sessions
```rust
#[test]
fn test_action_logging() {
    // All actions should be logged to audit trail
}
```

## Configuration

### Constants

```rust
// Maximum session duration in seconds (1 hour)
pub const MAX_SESSION_DURATION: u64 = 3600;

// Default session duration in seconds (15 minutes)
pub const DEFAULT_SESSION_DURATION: u64 = 900;

// Maximum concurrent sessions per maintainer
pub const MAX_CONCURRENT_SESSIONS: u32 = 5;
```

### Customization

To customize these values for your deployment:

1. Modify the constants in `shared/src/impersonation.rs`
2. Rebuild the shared crate
3. Update documentation to reflect new values
4. Test with new values to ensure proper operation

## Migration and Deployment

### No Database Migration Required

- Uses existing persistent storage patterns
- No schema changes required
- Session data stored in new storage keys (no conflicts)

### Role Addition

The `Support` role must be added to the system:

```rust
// Grant Support role to authorized maintainers
grant_role(&env, &admin, &support_address, Role::Support)?;
```

### Permission Registration

The `ImpersonateUser` permission is automatically available through the existing permission system.

### Deployment Steps

1. Deploy updated shared crate with impersonation module
2. Grant `Support` role to authorized maintainers
3. Update monitoring to track impersonation events
4. Train maintainers on proper impersonation procedures
5. Review audit trails after initial deployment

## Troubleshooting

### Session Creation Fails

**Problem**: `create_session()` returns `Unauthorized`

**Solutions**:
- Verify caller holds `Support` role
- Check that `Permission::ImpersonateUser` is properly configured
- Ensure target user is not an admin

### Actions Blocked During Session

**Problem**: `check_action_permission()` returns `ActionNotPermitted`

**Solutions**:
- Verify the action type matches the session scope
- Check resource scoping if using resource-level restrictions
- Consider escalating scope if action is legitimate

### Dangerous Mutation Blocked

**Problem**: `check_action_permission()` returns `DangerousMutationBlocked`

**Solutions**:
- Verify the action is truly necessary for debugging
- If required, recreate session with `allow_dangerous_mutations: true`
- Consider alternative debugging approaches

### Session Expires Unexpectedly

**Problem**: Session expires before expected time

**Solutions**:
- Check ledger timestamp vs expected expiration
- Verify session duration was set correctly
- Check for ledger time synchronization issues

### Too Many Active Sessions

**Problem**: `create_session()` returns `TooManyActiveSessions`

**Solutions**:
- Revoke unused or completed sessions
- Wait for existing sessions to expire
- Consider increasing `MAX_CONCURRENT_SESSIONS` if needed

## Monitoring and Alerting

### Key Events to Monitor

1. **Session Creation**: `impersonation.session_created`
   - Alert on unusual patterns (e.g., many sessions in short time)
   - Monitor for impersonation of sensitive users

2. **Session Revocation**: `impersonation.session_revoked`
   - Track revocation patterns
   - Alert on frequent revocations

3. **Action Recording**: `impersonation.action_recorded`
   - Monitor for dangerous actions during impersonation
   - Track action counts per session

4. **Session Expiration**: Auto-expiration not explicitly evented
   - Monitor session duration patterns
   - Alert on sessions running to expiration

### Recommended Alerts

- **High session creation rate**: May indicate abuse or system issues
- **Dangerous actions during impersonation**: Requires immediate review
- **Sessions targeting admin users**: Should never happen
- **Long-running sessions**: May indicate incomplete debugging

## API Reference

### Core Functions

#### `create_session`

Creates a new impersonation session.

```rust
pub fn create_session(
    env: &Env,
    impersonator: &Address,
    params: SessionParams,
) -> Result<SessionResult, ImpersonationError>
```

#### `validate_session`

Validates and returns an active session if it exists and is not expired.

```rust
pub fn validate_session(
    env: &Env,
    session_id: u64,
) -> Result<ImpersonationSession, ImpersonationError>
```

#### `revoke_session`

Revokes an active impersonation session.

```rust
pub fn revoke_session(
    env: &Env,
    caller: &Address,
    session_id: u64,
) -> Result<(), ImpersonationError>
```

#### `check_action_permission`

Checks if an action is permitted within the session scope.

```rust
pub fn check_action_permission(
    env: &Env,
    session: &ImpersonationSession,
    action_type: ImpersonationAction,
    resource_type: ResourceType,
    resource_id: Option<u64>,
) -> Result<(), ImpersonationError>
```

#### `record_action`

Records an action taken during an impersonation session.

```rust
pub fn record_action(
    env: &Env,
    session_id: u64,
    action_type: ImpersonationAction,
    resource_type: ResourceType,
    resource_id: Option<u64>,
) -> Result<(), ImpersonationError>
```

### Query Functions

#### `is_being_impersonated`

Checks if a user is currently being impersonated.

```rust
pub fn is_being_impersonated(env: &Env, target_user: &Address) -> bool
```

#### `get_current_impersonation`

Gets the current impersonation session for a target user (if any).

```rust
pub fn get_current_impersonation(
    env: &Env,
    target_user: &Address,
) -> Option<ImpersonationSession>
```

#### `get_active_sessions_for_user`

Gets active impersonation sessions for a target user.

```rust
pub fn get_active_sessions_for_user(
    env: &Env,
    target_user: &Address,
) -> Vec<ImpersonationSession>
```

#### `get_active_sessions_for_impersonator`

Gets active impersonation sessions for an impersonator.

```rust
pub fn get_active_sessions_for_impersonator(
    env: &Env,
    impersonator: &Address,
) -> Vec<ImpersonationSession>
```

## Data Structures

### ImpersonationSession

Complete session record with full metadata:

```rust
pub struct ImpersonationSession {
    pub session_id: u64,
    pub impersonator: Address,
    pub target_user: Address,
    pub scope: ImpersonationScope,
    pub resource_scopes: Vec<ResourceScope>,
    pub created_at: u64,
    pub expires_at: u64,
    pub allow_dangerous_mutations: bool,
    pub reason: Symbol,
    pub state: SessionState,
    pub action_count: u32,
}
```

### SessionParams

Parameters for creating a new session:

```rust
pub struct SessionParams {
    pub target_user: Address,
    pub scope: ImpersonationScope,
    pub resource_scopes: Vec<ResourceScope>,
    pub duration_sec: u64,
    pub allow_dangerous_mutations: bool,
    pub reason: Symbol,
}
```

### ImpersonationScope

Permission scope for the session:

```rust
pub enum ImpersonationScope {
    ReadOnly,
    Diagnostic,
    SafeMutations,
    FullAccess,
}
```

### ResourceType

Types of resources that can be scoped:

```rust
pub enum ResourceType {
    Aid,
    Payment,
    Escrow,
    Treasury,
    Roles,
    Config,
    All,
}
```

### ImpersonationAction

Types of actions that can be taken:

```rust
pub enum ImpersonationAction {
    Read,
    Diagnostic,
    SafeMutation,
    DangerousMutation,
    ConfigChange,
    RoleManagement,
    TreasuryOperation,
}
```

## Security Considerations

### Privilege Escalation Prevention

- Admin users cannot be impersonated
- Users with sensitive roles (TreasuryManager, etc.) cannot be impersonated
- Role hierarchy is respected during impersonation

### Data Protection

- Resource scoping limits access to specific data
- Read-only scope prevents any data modification
- Audit trail records all data access

### Action Accountability

- Every action is logged with full context
- Session metadata includes impersonator identity
- Action count tracking helps identify abuse patterns

### Time-Bounded Access

- Sessions automatically expire
- Maximum duration prevents long-term access
- Manual revocation available for immediate termination

## Future Enhancements

Potential future improvements:

1. **Session Templates**: Pre-configured session templates for common debugging scenarios
2. **Approval Workflow**: Require admin approval for high-scope sessions
3. **Session Renewal**: Allow session renewal with proper authorization
4. **Geographic Restrictions**: Limit impersonation based on geographic criteria
5. **Rate Limiting**: Add time-based rate limiting for session creation
6. **Multi-Factor Authentication**: Require additional auth for high-risk sessions

## Contributing

When contributing to the impersonation module:

1. Maintain backward compatibility with existing authorization system
2. Add tests for new features or scope types
3. Update this documentation for API changes
4. Follow existing code style and patterns
5. Ensure security considerations are maintained
6. Add audit events for any new operations

## License

This module is part of the Trellis Contracts project and follows the same license terms.
