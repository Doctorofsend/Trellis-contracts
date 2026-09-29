# Partial Failure Dashboard

## Overview

The Partial Failure Dashboard provides maintainers with a comprehensive view of operations that are stuck between internal contract state and external systems. This enables quick resolution of user-impacting failures by tracking partially completed operations and external reference IDs.

## Features

### 1. Partial Failure Tracking

The dashboard tracks operations that have partially completed but failed before full completion, including:

- **Payment operations**: Failed or stuck payment/escrow transactions
- **Transfer operations**: Token transfers that didn't complete
- **Worker jobs**: Background tasks that failed or got stuck
- **Webhook deliveries**: Failed external webhook calls
- **External API integrations**: Failed third-party API calls
- **Database synchronization**: Failed data sync operations
- **Contract operations**: Other on-chain operation failures

### 2. External Reference Tracking

Each partial failure can include external reference IDs for cross-system tracking:

```rust
add_external_reference(
    &mut failure,
    &env,
    symbol_short!("stripe"),
    String::from_str(&env, "pi_1234567890"),
);
```

Supported external systems include:
- Payment processors (Stripe, PayPal, etc.)
- Webhook endpoints
- External APIs
- Database systems
- Other off-chain services

### 3. Failure Classification

#### Severity Levels

- **Critical**: User funds or critical data at risk (e.g., unauthorized access, insufficient balance)
- **High**: User experience degraded but funds safe (e.g., payment/transfer failures)
- **Medium**: Background task failed, no immediate user impact (e.g., stale worker jobs)
- **Low**: Low-priority or informational issues

#### Operation Types

- `Payment`: Payment or escrow operations
- `Transfer`: Token transfers or swaps
- `Worker`: Background worker jobs
- `Webhook`: Webhook delivery
- `ExternalApi`: External API integration
- `DatabaseSync`: Database synchronization
- `Contract`: Other contract operations

#### Failure States

- `Unresolved`: Failure occurred and is pending resolution
- `Retryable`: Failure is retryable and will be attempted again
- `Ignored`: Failure has been manually ignored by operator
- `Resolved`: Failure has been resolved

### 4. Failure Grouping

The dashboard groups failures by multiple dimensions for easier analysis:

#### By Operation Type
Groups failures by the type of operation (Payment, Transfer, Worker, etc.)

#### By Severity
Groups failures by severity level (Critical, High, Medium, Low)

#### By Age
Groups failures as:
- **Fresh**: Failures less than 1000 ledgers old
- **Stale**: Failures 1000+ ledgers old (require attention)

#### By Retryability
Groups failures as:
- **Retryable**: Can be automatically retried
- **Non-retryable**: Require manual intervention

### 5. Actionable Links

Each failure includes actionable links for remediation:

- **Retry Link**: `api/retry/{id}` - Retry the operation (if retryable)
- **Inspect Link**: `api/inspect/{id}` - View detailed failure information
- **Remediation Link**: Documentation for manual remediation based on failure state

## Usage

### Creating a Partial Failure

```rust
use trellis_contracts::{
    create_partial_failure, add_external_reference,
    OperationType, OperationKind, FailureState,
};

// Create a new partial failure record
let mut failure = create_partial_failure(
    &env,
    failure_id,           // Unique identifier
    OperationType::Payment,
    operation_id,        // Correlation ID or job ID
    error_code,          // Error that caused the failure
    failed_at_ledger,    // Ledger when failure occurred
    OperationKind::WalletAction,
    String::from_str(&env, "Payment failed due to authorization"),
);

// Add external reference for cross-system tracking
add_external_reference(
    &mut failure,
    &env,
    symbol_short!("stripe"),
    String::from_str(&env, "pi_1234567890"),
);
```

### Generating the Dashboard

```rust
use trellis_contracts::{
    generate_enhanced_dashboard, PartialFailure,
};

// Collect partial failures from your system
let mut partial_failures = Vec::new(&env);
// ... add failures ...

// Generate the enhanced dashboard
let report = generate_enhanced_dashboard(
    &env,
    ledger_records,
    db_records,
    user_records,
    current_timestamp,
    partial_failures,
);

// Access dashboard data
for failure in report.partial_failures.iter() {
    println!("Failure {}: {} (Severity: {:?})", 
        failure.failure_id, 
        failure.description, 
        failure.severity
    );
}

// Access grouped failures
for group in report.failure_groups.iter() {
    println!("{} {}: {} failures", 
        group.dimension, 
        group.value, 
        group.count
    );
}
```

### Manually Managing Failures

```rust
// Mark a failure as ignored
failure.state = FailureState::Ignored;

// Mark a failure as resolved
failure.state = FailureState::Resolved;

// Resolved failures are automatically excluded from the dashboard
```

## Security Considerations

### Data Redaction

The dashboard automatically redacts sensitive information:

- External reference IDs are hidden in the redacted view
- Only system names (e.g., "stripe") are exposed, not actual IDs
- Full reference IDs are only available to authorized maintainers

### Access Control

The dashboard respects the existing authorization system:

- Only maintainers with appropriate roles can view detailed failure information
- The redacted view is safe for broader access
- Remediation actions require admin authorization

## Testing

The implementation includes comprehensive tests covering:

- **Stale failures**: Failures older than 1000 ledgers
- **Retryable failures**: Failures that can be automatically retried
- **Resolved failures**: Failures that have been resolved (excluded from dashboard)
- **Manually ignored failures**: Failures marked as ignored by operators

Run tests with:

```bash
cargo test --package shared test_dashboard
```

## Integration Points

### Existing Modules

The partial failure dashboard integrates with existing Trellis Contracts modules:

- **Jobs Module**: Tracks dead-lettered worker jobs
- **Health Module**: Monitors external dependency health
- **Reconciliation Module**: Detects data drift between systems
- **Recovery Module**: Provides operation recovery checkpoints
- **Events Module**: Correlates failures with ledger events

### Error Classification

The dashboard uses the existing error classification from the jobs module:

```rust
use trellis_contracts::is_retryable_error;

let is_retryable = is_retryable_error(error_code);
```

Error codes are classified as retryable or non-retryable based on the shared error taxonomy.

## Design Decisions

### 1. Storage vs In-Memory

Partial failures are designed to be stored in persistent storage for long-term tracking, but the dashboard generation is stateless. This allows:

- Flexible storage backends (contract storage, off-chain database, etc.)
- Real-time dashboard generation without storage dependencies
- Easy testing and simulation

### 2. Severity Classification

Severity is determined by a combination of:
- Error code (critical errors like Unauthorized are always Critical)
- Operation type (Payment/Transfer operations are High severity)
- Age (stale failures are bumped to Medium severity)

This multi-factor approach ensures maintainers focus on the most impactful failures first.

### 3. External Reference IDs

External references are stored separately from the main failure record to:
- Enable cross-system correlation without leaking secrets
- Support multiple external systems per failure
- Allow flexible reference ID formats

### 4. Grouping Strategy

Failures are grouped by multiple dimensions to support different workflows:
- **Operation type**: For team-based routing (payment team, worker team, etc.)
- **Severity**: For triage and prioritization
- **Age**: For identifying stale issues that need attention
- **Retryability**: For automated vs manual remediation

## API Reference

### Core Functions

#### `create_partial_failure`

Creates a new partial failure record with automatic severity and retryability classification.

```rust
pub fn create_partial_failure(
    env: &Env,
    failure_id: u64,
    operation_type: OperationType,
    operation_id: u64,
    error_code: u32,
    failed_at_ledger: u32,
    operation_kind: OperationKind,
    description: String,
) -> PartialFailure
```

#### `add_external_reference`

Adds an external reference ID to a partial failure for cross-system tracking.

```rust
pub fn add_external_reference(
    env: &Env,
    failure: &mut PartialFailure,
    system: Symbol,
    reference_id: String,
)
```

#### `redact_partial_failure`

Redacts sensitive information from a partial failure for dashboard display.

```rust
pub fn redact_partial_failure(
    env: &Env,
    failure: &PartialFailure,
) -> RedactedPartialFailure
```

#### `generate_enhanced_dashboard`

Generates a comprehensive dashboard report with partial failure tracking and grouping.

```rust
pub fn generate_enhanced_dashboard(
    env: &Env,
    ledger_records: Vec<SourceRecord>,
    db_records: Vec<SourceRecord>,
    user_records: Vec<SourceRecord>,
    current_timestamp: u64,
    partial_failures: Vec<PartialFailure>,
) -> DashboardReport
```

### Grouping Functions

#### `group_by_operation_type`

Groups failures by operation type (Payment, Transfer, Worker, etc.)

```rust
pub fn group_by_operation_type(
    env: &Env,
    failures: &Vec<PartialFailure>,
) -> Vec<FailureGroup>
```

#### `group_by_severity`

Groups failures by severity level (Critical, High, Medium, Low)

```rust
pub fn group_by_severity(
    env: &Env,
    failures: &Vec<PartialFailure>,
) -> Vec<FailureGroup>
```

#### `group_by_age`

Groups failures by age (fresh vs stale)

```rust
pub fn group_by_age(
    env: &Env,
    failures: &Vec<PartialFailure>,
) -> Vec<FailureGroup>
```

#### `group_by_retryability`

Groups failures by retryability (retryable vs non-retryable)

```rust
pub fn group_by_retryability(
    env: &Env,
    failures: &Vec<PartialFailure>,
) -> Vec<FailureGroup>
```

## Data Structures

### PartialFailure

Complete failure record with full metadata:

```rust
pub struct PartialFailure {
    pub failure_id: u64,
    pub operation_type: OperationType,
    pub state: FailureState,
    pub severity: FailureSeverity,
    pub operation_id: u64,
    pub external_refs: Vec<ExternalReference>,
    pub error_code: u32,
    pub failed_at_ledger: u32,
    pub retry_attempts: u32,
    pub is_retryable: bool,
    pub operation_kind: OperationKind,
    pub age_ledgers: u32,
    pub description: String,
}
```

### RedactedPartialFailure

Redacted version for dashboard display (no secrets):

```rust
pub struct RedactedPartialFailure {
    pub failure_id: u64,
    pub operation_type: OperationType,
    pub state: FailureState,
    pub severity: FailureSeverity,
    pub operation_id: u64,
    pub external_systems: Vec<Symbol>,  // Only system names, not IDs
    pub error_code: u32,
    pub failed_at_ledger: u32,
    pub retry_attempts: u32,
    pub is_retryable: bool,
    pub age_ledgers: u32,
    pub description: String,
    pub retry_link: String,
    pub inspect_link: String,
    pub remediation_link: String,
}
```

### FailureGroup

Grouped failure summary:

```rust
pub struct FailureGroup {
    pub dimension: Symbol,        // "operation_type", "severity", "age", "retryability"
    pub value: Symbol,           // "payment", "critical", "stale", "retryable"
    pub count: u32,              // Total failures in this group
    pub critical_count: u32,     // Critical failures in this group
    pub retryable_count: u32,    // Retryable failures in this group
    pub investigation_link: String,
}
```

## Maintenance

### Adding New Operation Types

To add a new operation type:

1. Add the variant to `OperationType` enum in `dashboard.rs`
2. Add the symbol mapping in `operation_type_to_symbol()`
3. Update the operation types array in `group_by_operation_type()`
4. Add tests for the new operation type

### Adding New Severity Levels

To add a new severity level:

1. Add the variant to `FailureSeverity` enum in `dashboard.rs`
2. Add the symbol mapping in `severity_to_symbol()`
3. Update the severity array in `group_by_severity()`
4. Update `classify_severity()` to use the new level
5. Add tests for the new severity level

### Adjusting Stale Threshold

The stale threshold is currently set to 1000 ledgers. To adjust:

```rust
// In group_by_age()
const STALE_THRESHOLD: u32 = 1000; // Change this value
```

## Troubleshooting

### Failures Not Appearing in Dashboard

If failures aren't appearing in the dashboard:

1. Check that the failure state is not `Resolved` (resolved failures are excluded)
2. Verify the failure is being added to the `partial_failures` vector
3. Ensure `generate_enhanced_dashboard()` is being called with the correct parameters

### Incorrect Severity Classification

If severity classification seems incorrect:

1. Check the error code against the classification logic in `classify_severity()`
2. Verify the operation type is set correctly
3. Consider the age of the failure (stale failures are bumped to Medium)

### External References Not Showing

If external references aren't appearing in the redacted view:

1. Ensure `add_external_reference()` is being called before redaction
2. Verify the system name is a valid Symbol
3. Check that the reference ID is not empty

## Future Enhancements

Potential future improvements:

1. **Storage Integration**: Add persistent storage for partial failures
2. **Automated Retry**: Integrate with the jobs module for automatic retry
3. **Alerting**: Add alerting rules for critical failures
4. **Historical Trends**: Track failure patterns over time
5. **Custom Grouping**: Allow custom grouping dimensions
6. **Remediation Automation**: Automated remediation for common failure patterns

## Contributing

When contributing to the partial failure dashboard:

1. Maintain backward compatibility with existing dashboard functionality
2. Add tests for new features
3. Update this documentation for API changes
4. Follow the existing code style and patterns
5. Ensure security considerations are maintained (no secret leakage)

## License

This module is part of the Trellis Contracts project and follows the same license terms.
