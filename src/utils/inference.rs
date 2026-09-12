/// Bound each background model to two compute threads; use one on a small CPU.
/// This is not affinity or a process-wide reservation: the OS still schedules all work.
pub fn thread_budget(available_cpus: usize) -> usize {
    available_cpus.saturating_sub(1).clamp(1, 2)
}

#[cfg(test)]
#[path = "../../tests/utils/inference_tests.rs"]
mod tests;
