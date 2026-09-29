## Summary
- Implemented comprehensive scoped maintainer impersonation system for support debugging
- Added time-limited, scoped, and fully audited impersonation sessions
- Implemented dangerous mutation blocking with explicit permission requirement
- Added visible indicators through ledger events and session state tracking
- Integrated with existing authorization, timeline, and audit trail systems

## Background
Maintainers need a safe way to reproduce user-reported issues without gaining broad access to private data or performing irreversible actions. This implementation provides a secure, time-limited, and fully audited impersonation system that enables safe debugging while maintaining strict security boundaries.

## Changes Made

### Core Implementation
- **Created `shared/src/impersonation.rs`**:
  - `ImpersonationSession` structure with session metadata (session_id, impersonator, target_user, scope, resource_scopes, duration, state, action_count)
  - `SessionParams` for session creation with configurable scope and duration
  - `ImpersonationScope` enum (ReadOnly, Diagnostic, SafeMutations, FullAccess)
  - `ResourceType` enum (Aid, Payment, Escrow, Treasury, Roles, Config, All)
  - `ResourceScope` for resource-level access control
  - `ImpersonationAction` enum (Read, Diagnostic, SafeMutation, DangerousMutation, ConfigChange, RoleManagement, TreasuryOperation)
  - `SessionState` enum (Active, Revoked, Expired, Completed)
  - `ImpersonationError` enum (960-969) for impersonation-specific errors

### Session Management Functions
- `create_session()`: Creates new impersonation session with authorization checks
- `validate_session()`: Validates session is active and not expired
- `revoke_session()`: Manually revokes an active session
- `check_action_permission()`: Checks if action is permitted within session scope
- `record_action()`: Records action taken during impersonation with audit logging

### Query Functions
- `is_being_impersonated()`: Checks if user is currently being impersonated
- `get_current_impersonation()`: Gets current session for a target user
- `get_active_sessions_for_user()`: Gets all active sessions for a target user
- `get_active_sessions_for_impersonator()`: Gets all active sessions for an impersonator

### Authorization System Updates
- **Updated `shared/src/auth.rs`**:
  - Added `Role::Support` for impersonation authorization
  - Added `Permission::ImpersonateUser` for impersonation permission
  - Updated `role_for_permission()` to map `ImpersonateUser` to `Support` role

### Module Integration
- **Updated `shared/src/lib.rs`**:
  - Added `impersonation` module declaration
  - Exported all impersonation types and functions for public use
  - Maintained backward compatibility with existing exports

### Security Features
- **Time-Limited Sessions**: Default 15 minutes, maximum 1 hour
- **Scoped Permissions**: ReadOnly, Diagnostic, SafeMutations, FullAccess
- **Resource-Level Scoping**: Limit access to specific resource types and IDs
- **Dangerous Mutation Blocking**: Automatic blocking unless explicitly allowed
- **Target Restrictions**: Admin users cannot be impersonated
- **Concurrent Session Limits**: Maximum 5 sessions per impersonator
- **Automatic Expiration**: Sessions auto-expire when time limit is reached

### Audit Logging
- **Session Creation**: Logged to action audit trail with full context
- **Session Revocation**: Logged with revoker identity and reason
- **Action Recording**: Every action during impersonation is logged
- **Ledger Events**: Events emitted for session_created, session_revoked, action_recorded
- **Integration**: Uses existing `record_action_audit_event()` and timeline system

### Testing
- **Comprehensive Test Suite** (10 test functions):
  - Basic session creation with proper authorization
  - Unauthorized session creation prevention
  - Duration limit enforcement
  - Session expiration handling
  - Read-only scope blocking mutations
  - Dangerous mutation blocking
  - Session revocation functionality
  - Resource scope filtering
  - Active session detection
  - Concurrent session limit enforcement

### Documentation
- **Created `docs/SCOPED_IMPERSONATION.md`**:
  - Complete feature overview and usage guide
  - Security model and authorization requirements
  - API reference with function signatures
  - Integration points with existing systems
  - Best practices for maintainers and developers
  - Troubleshooting guide
  - Monitoring and alerting recommendations
  - Configuration and deployment instructions

## Design Decisions

### 1. Permission-Based Authorization
- Uses existing permission system (`Permission::ImpersonateUser`)
- Maps to new `Role::Support` for clean separation
- Consistent with existing RBAC patterns
- Allows future role expansions without code changes

### 2. Multi-Layer Security
- Time limits prevent long-term access
- Scope restrictions limit what can be done
- Resource scoping limits what can be accessed
- Dangerous mutation blocking prevents irreversible actions
- Multiple independent security controls

### 3. Audit Trail Integration
- Leverages existing timeline and audit infrastructure
- Consistent event patterns with existing system
- No new storage schemas required
- Maintains unified audit view across all operations

### 4. Flexible Scoping Model
- Four permission scopes for different use cases
- Resource-level scoping for precision
- Dangerous mutation flag for explicit control
- Allows escalation without session recreation

### 5. Session State Management
- Clear state transitions (Active → Revoked/Expired/Completed)
- Automatic cleanup of expired sessions
- Index-based queries for performance
- Prevents stale session accumulation

## Test Plan

### Unit Tests
- ✅ Session creation with Support role authorization
- ✅ Unauthorized session creation prevention
- ✅ Duration limit enforcement (MAX_SESSION_DURATION)
- ✅ Session expiration after time limit
- ✅ Read-only scope blocking mutation actions
- ✅ Dangerous mutation blocking without explicit permission
- ✅ Session revocation by creator or authorized party
- ✅ Resource scope filtering for precise access control
- ✅ Active session detection and querying
- ✅ Concurrent session limit enforcement

### Integration Tests
- ✅ Permission system integration with Support role
- ✅ Timeline audit trail integration
- ✅ Event emission and monitoring
- ✅ Storage persistence and retrieval
- ✅ State transitions and cleanup

## Breaking Changes
None. This is a pure addition to the existing authorization system with backward compatibility maintained.

## Migration Steps
1. Deploy updated shared crate with impersonation module
2. Grant `Support` role to authorized maintainers
3. Update monitoring to track impersonation events
4. Train maintainers on proper impersonation procedures
5. Review audit trails after initial deployment

## Configuration
- `MAX_SESSION_DURATION`: 3600 seconds (1 hour) - maximum session duration
- `DEFAULT_SESSION_DURATION`: 900 seconds (15 minutes) - default session duration
- `MAX_CONCURRENT_SESSIONS`: 5 - maximum concurrent sessions per impersonator

## Deployment Notes
- No database schema changes required
- No contract storage changes (uses new storage keys)
- Documentation included for maintainers
- Tests can be run with: `cargo test --package shared test_impersonation`
- Monitor impersonation events for initial deployment

Generated with [Devin](https://devin.ai)
