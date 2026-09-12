/// Bound each background model to two compute threads; use one on a small CPU.
/// This is not affinity or a process-wide reservation: the OS still schedules all work.
pub fn thread_budget(available_cpus: usize) -> usize {
    available_cpus.saturating_sub(1).clamp(1, 2)
}

#[cfg(test)]
mod tests {
    use super::thread_budget;

    #[test]
    fn background_budget_is_nonzero_and_leaves_room_on_small_cpus() {
        for (available, expected) in [(0, 1), (1, 1), (2, 1), (3, 2), (4, 2), (64, 2)] {
            assert_eq!(thread_budget(available), expected);
        }
        assert_eq!(thread_budget(usize::MAX), 2);
    }
}
