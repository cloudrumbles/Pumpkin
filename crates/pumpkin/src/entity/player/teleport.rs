use std::sync::atomic::{AtomicI32, Ordering};

const fn increment(current: i32) -> i32 {
    if current == i32::MAX - 1 {
        0
    } else {
        current.wrapping_add(1)
    }
}

pub(super) fn next_id(counter: &AtomicI32) -> i32 {
    let mut current = counter.load(Ordering::Relaxed);
    loop {
        let next = increment(current);
        match counter.compare_exchange_weak(current, next, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => return next,
            Err(actual) => current = actual,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::next_id;
    use std::sync::atomic::{AtomicI32, Ordering};

    #[test]
    fn first_request_uses_one() {
        let counter = AtomicI32::new(0);
        assert_eq!(next_id(&counter), 1);
        assert_eq!(counter.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn wraps_before_the_maximum_id_like_vanilla() {
        let counter = AtomicI32::new(i32::MAX - 2);
        assert_eq!(next_id(&counter), i32::MAX - 1);
        assert_eq!(next_id(&counter), 0);
        assert_eq!(counter.load(Ordering::Relaxed), 0);
        assert_eq!(next_id(&counter), 1);
    }
}
