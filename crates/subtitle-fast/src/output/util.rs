use std::time::Duration;

pub(crate) fn duration_millis(duration: Duration) -> u64 {
    let millis = duration.as_millis();
    if millis > u64::MAX as u128 {
        u64::MAX
    } else {
        millis as u64
    }
}

pub(crate) fn duration_secs(duration: Duration) -> f64 {
    duration.as_secs_f64()
}
