//! Legacy failure allowances granted by removed operator retry commands.
use super::*;

impl Store {
    /// A retry contributes exactly one failure allowance, never erases failures.
    /// Successful later stages can proceed, but another failure reaches the new bound.
    pub fn effective_provider_failure_limit(&self, case_key: &str, base: u64) -> Result<u64> {
        let granted: Option<i64> = self.connection.query_row(
            "SELECT MAX(json_extract(payload_json,'$.failed_attempts')+1) FROM events
             WHERE case_key=?1 AND ((event_type='BUILDER_RETRY_AUTHORIZED'
               AND previous_state IN ('READY_TO_BUILD','ESCALATED') AND next_state='READY_TO_BUILD')
               OR (event_type='REVIEW_RETRY_AUTHORIZED' AND previous_state='ESCALATED' AND next_state='WAITING_CI')
               OR (event_type='PLANNER_RETRY_AUTHORIZED' AND previous_state='ESCALATED' AND next_state='PLANNING'))",
            [case_key],
            |row| row.get(0),
        )?;
        Ok(base.max(granted.map(unsigned).unwrap_or(0)))
    }
}
