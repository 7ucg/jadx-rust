//! Small thread pool helper: scoped worker threads pulling item indexes from a shared counter.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Run `work(state, index)` for every index in `0..count` on up to `threads` threads. Every
/// thread creates its own state with `init`. Items are handed out in increasing order; the
/// completion order is not defined, so `work` must write to per item destinations.
pub fn run_indexed<S, I, W>(count: usize, threads: usize, init: I, work: W)
where
    I: Fn() -> S + Sync,
    W: Fn(&mut S, usize) + Sync,
{
    if count == 0 {
        return;
    }
    let threads = threads.clamp(1, count);
    if threads == 1 {
        let mut state = init();
        for i in 0..count {
            work(&mut state, i);
        }
        return;
    }
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..threads {
            scope.spawn(|| {
                let mut state = init();
                loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    if i >= count {
                        break;
                    }
                    work(&mut state, i);
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn visits_every_item_once() {
        let seen = Mutex::new(vec![0u32; 100]);
        run_indexed(
            100,
            4,
            || (),
            |_, i| {
                seen.lock().unwrap()[i] += 1;
            },
        );
        assert!(seen.lock().unwrap().iter().all(|&n| n == 1));
    }

    #[test]
    fn empty_and_single() {
        run_indexed(0, 8, || (), |_, _| panic!("no items"));
        let n = AtomicUsize::new(0);
        run_indexed(
            3,
            1,
            || (),
            |_, _| {
                n.fetch_add(1, Ordering::Relaxed);
            },
        );
        assert_eq!(n.load(Ordering::Relaxed), 3);
    }
}
