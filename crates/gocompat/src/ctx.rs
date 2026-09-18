//! Go `context.Context` as [`Ctx`]: cancellation, deadlines that nest, and Go's error texts
//! (go1.26.5 `context/context.go`).
//!
//! Semantics:
//! - [`Ctx::background`] never ends on its own. [`Ctx::cancel`] works on every `Ctx`, including a
//!   background one (the shape of `signal.NotifyContext(context.Background(), …)`).
//! - A child ([`Ctx::with_cancel`], [`Ctx::with_timeout`]) ends when its parent ends, with the
//!   parent's error. A child created from a parent that has already ended has ended too.
//! - `with_timeout(d)` sets the deadline to `min(parent deadline, now + d)`: Go's `WithDeadline` returns
//!   a plain `WithCancel` child when the parent's deadline comes first. A zero timeout has already ended
//!   when it returns, with `context deadline exceeded`.
//! - `cancel()` ends this ctx, its clones and its descendants, never its parent. Cancelling an ended
//!   ctx changes nothing.
//! - [`Ctx::err`] is the cause of the first event (own cancel, own deadline, or the parent's end) and
//!   never changes afterwards.
//!
//! Implementation:
//! - Cancellation travels down a `CancellationToken` tree. Every `cancel()` first records the instant
//!   it happened, then cancels the token.
//! - Deadlines are compared with the tokio clock (`tokio::time::Instant`, so `tokio::time::pause`
//!   applies). No timer task is spawned, so a `Ctx` can be created and inspected outside a runtime.
//! - The first event is latched per node when it is first observed. Parent and child then agree on
//!   the cause even when a cancel races a deadline.

use std::fmt;
use std::sync::{Arc, OnceLock};

use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// A Go `context.Context`. Clones share one context: cancelling a clone cancels them all.
#[derive(Clone)]
pub struct Ctx {
    node: Arc<Node>,
}

/// `context.Canceled` / `context.DeadlineExceeded`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum CtxError {
    #[error("context canceled")]
    Canceled,
    #[error("context deadline exceeded")]
    DeadlineExceeded,
}

/// One context of the tree.
struct Node {
    /// Cancelled by `cancel()` of this node or of an ancestor.
    token: CancellationToken,
    /// The effective deadline: the minimum of this node's own deadline and its ancestors'.
    deadline: Option<Instant>,
    /// When `cancel()` was first called on this node.
    canceled_at: OnceLock<Instant>,
    /// The first event, once observed.
    first: OnceLock<Event>,
    parent: Option<Arc<Node>>,
}

/// The end of a context: its error and when it happened.
#[derive(Clone, Copy)]
struct Event {
    err: CtxError,
    at: Instant,
}

impl Node {
    fn root() -> Node {
        Node {
            token: CancellationToken::new(),
            deadline: None,
            canceled_at: OnceLock::new(),
            first: OnceLock::new(),
            parent: None,
        }
    }

    /// The first event of this node as of `now`, latching it (and the ancestors' events it depends on)
    /// once it exists.
    fn first_event(&self, now: Instant) -> Option<Event> {
        if let Some(ev) = self.first.get() {
            return Some(*ev);
        }
        // Every event cancels this node's token or passes its effective deadline.
        let deadline_passed = self.deadline.is_some_and(|d| now >= d);
        if !deadline_passed && !self.token.is_cancelled() {
            return None;
        }
        // Resolve top-down from the nearest latched ancestor (or the root), iteratively so deep
        // chains cannot exhaust the stack.
        let mut chain: Vec<&Node> = vec![self];
        let mut inherited: Option<Event> = None;
        let mut cur = self.parent.as_deref();
        while let Some(n) = cur {
            if let Some(ev) = n.first.get() {
                inherited = Some(*ev);
                break;
            }
            chain.push(n);
            cur = n.parent.as_deref();
        }
        for n in chain.into_iter().rev() {
            inherited = n.resolve(inherited, now);
        }
        inherited
    }

    /// This node's first event given its parent's (`parent`), as of `now`.
    fn resolve(&self, parent: Option<Event>, now: Instant) -> Option<Event> {
        if let Some(ev) = self.first.get() {
            return Some(*ev);
        }
        let mut best = parent;
        if let Some(at) = self.canceled_at.get() {
            best = Some(earliest(
                best,
                Event {
                    err: CtxError::Canceled,
                    at: *at,
                },
            ));
        }
        if let Some(d) = self.deadline
            && now >= d
        {
            best = Some(earliest(
                best,
                Event {
                    err: CtxError::DeadlineExceeded,
                    at: d,
                },
            ));
        }
        best.map(|ev| *self.first.get_or_init(|| ev))
    }
}

/// The earlier of two events. At the same instant the deadline wins: a context is done at its
/// deadline, so a cancel at that very instant comes too late.
fn earliest(a: Option<Event>, b: Event) -> Event {
    match a {
        None => b,
        Some(a) if b.at < a.at || (b.at == a.at && b.err == CtxError::DeadlineExceeded) => b,
        Some(a) => a,
    }
}

impl Ctx {
    /// `context.Background()`.
    pub fn background() -> Ctx {
        Ctx {
            node: Arc::new(Node::root()),
        }
    }

    /// `context.WithCancel`: a child; parent cancellation propagates.
    pub fn with_cancel(&self) -> Ctx {
        self.child(self.node.deadline)
    }

    /// `context.WithTimeout`: a child whose deadline is min(parent deadline, now + d).
    pub fn with_timeout(&self, d: std::time::Duration) -> Ctx {
        let own = Instant::now().checked_add(d);
        let deadline = match (self.node.deadline, own) {
            (Some(p), Some(o)) => Some(p.min(o)),
            // A deadline beyond the clock's range never comes (Go's is at most 292 years away).
            (p, None) => p,
            (None, o) => o,
        };
        self.child(deadline)
    }

    fn child(&self, deadline: Option<Instant>) -> Ctx {
        let now = Instant::now();
        let node = Node {
            token: self.node.token.child_token(),
            deadline,
            canceled_at: OnceLock::new(),
            first: OnceLock::new(),
            parent: Some(Arc::clone(&self.node)),
        };
        match self.node.first_event(now) {
            // Go's propagateCancel: a child of an ended parent ends at once with the parent's error,
            // before its own deadline is considered.
            Some(ev) => {
                let _ = node.first.set(ev);
            }
            // A deadline that has already passed (zero timeout) ends the child at creation.
            None => {
                let _ = node.first_event(now);
            }
        }
        Ctx {
            node: Arc::new(node),
        }
    }

    /// Cancels this ctx and its descendants only.
    pub fn cancel(&self) {
        let n = &self.node;
        if n.first.get().is_none() {
            let _ = n.canceled_at.set(Instant::now());
        }
        // Record first, then cancel: whoever sees the token cancelled finds the instant.
        n.token.cancel();
    }

    /// `Context.Deadline`.
    pub fn deadline(&self) -> Option<tokio::time::Instant> {
        self.node.deadline
    }

    /// `Context.Err`: the cause of the first event (own cancel, own deadline, or the parent's err).
    pub fn err(&self) -> Option<CtxError> {
        self.node.first_event(Instant::now()).map(|ev| ev.err)
    }

    /// `<-Context.Done()`.
    pub async fn done(&self) {
        if self.err().is_some() {
            return;
        }
        let n = &self.node;
        match n.deadline {
            Some(d) => {
                tokio::select! {
                    () = n.token.cancelled() => {}
                    () = tokio::time::sleep_until(d) => {
                        // The timer fired: latch the deadline even if the clock reads a hair early.
                        let _ = n.first_event(Instant::now().max(d));
                    }
                }
            }
            None => n.token.cancelled().await,
        }
    }

    /// Runs `f` until it completes or the ctx ends (`select { done, f }`).
    ///
    /// An ended ctx returns its error without polling `f`. When `f` completes and the ctx ends at the
    /// same time, either result may come out, as with Go's `select`.
    pub async fn run<F: std::future::Future>(&self, f: F) -> Result<F::Output, CtxError> {
        if let Some(e) = self.err() {
            return Err(e);
        }
        tokio::select! {
            () = self.done() => Err(self.err_after_done()),
            out = f => Ok(out),
        }
    }

    /// Sleeps for `d` unless the ctx ends first (`select { <-time.After(d), <-ctx.Done() }`).
    /// `tokio::time::sleep` saturates durations beyond the clock's range.
    pub async fn sleep(&self, d: std::time::Duration) -> Result<(), CtxError> {
        self.run(tokio::time::sleep(d)).await
    }

    /// The error of a ctx whose `done()` has resolved.
    fn err_after_done(&self) -> CtxError {
        match self.err() {
            Some(e) => e,
            None if self.node.token.is_cancelled() => CtxError::Canceled,
            None => CtxError::DeadlineExceeded,
        }
    }
}

/// Like Go's `%v` of a context, reduced to what can be observed: the deadline and the error.
impl fmt::Debug for Ctx {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Ctx")
            .field("deadline", &self.node.deadline)
            .field("err", &self.err())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    /// Long enough that a deadline this far away does not fire during a test.
    const LONG: Duration = Duration::from_secs(3600);
    /// A deadline that fires quickly.
    const SHORT: Duration = Duration::from_millis(30);

    async fn wait_done(ctx: &Ctx) {
        if tokio::time::timeout(Duration::from_secs(10), ctx.done())
            .await
            .is_err()
        {
            panic!("ctx did not end within 10 s");
        }
    }

    async fn not_done_within(ctx: &Ctx, d: Duration) -> bool {
        tokio::time::timeout(d, ctx.done()).await.is_err()
    }

    #[test]
    fn error_texts() {
        assert_eq!(CtxError::Canceled.to_string(), "context canceled");
        assert_eq!(
            CtxError::DeadlineExceeded.to_string(),
            "context deadline exceeded"
        );
    }

    #[test]
    fn works_outside_a_runtime() {
        let bg = Ctx::background();
        assert_eq!(bg.err(), None);
        assert_eq!(bg.deadline(), None);
        let t = bg.with_timeout(LONG);
        assert!(t.deadline().is_some());
        assert_eq!(t.err(), None);
        t.cancel();
        assert_eq!(t.err(), Some(CtxError::Canceled));
        assert_eq!(bg.err(), None);
        assert_eq!(
            bg.with_timeout(Duration::ZERO).err(),
            Some(CtxError::DeadlineExceeded)
        );
    }

    #[tokio::test]
    async fn background_never_ends() {
        let bg = Ctx::background();
        assert_eq!(bg.err(), None);
        assert_eq!(bg.deadline(), None);
        assert!(not_done_within(&bg, SHORT).await);
        assert_eq!(bg.run(async { 5 }).await, Ok(5));
        assert_eq!(bg.sleep(Duration::from_millis(1)).await, Ok(()));
        assert_eq!(bg.err(), None);
    }

    #[tokio::test]
    async fn cancel_wakes_done_and_sets_canceled() {
        let bg = Ctx::background();
        let c = bg.with_cancel();
        let waiter = {
            let c = c.clone();
            tokio::spawn(async move {
                c.done().await;
                c.err()
            })
        };
        tokio::time::sleep(Duration::from_millis(5)).await;
        c.cancel();
        match waiter.await {
            Ok(err) => assert_eq!(err, Some(CtxError::Canceled)),
            Err(e) => panic!("waiter failed: {e}"),
        }
        assert_eq!(c.err(), Some(CtxError::Canceled));
        assert_eq!(bg.err(), None, "cancel never reaches the parent");
        // Cancelling again changes nothing.
        c.cancel();
        assert_eq!(c.err(), Some(CtxError::Canceled));
    }

    #[tokio::test]
    async fn cancel_affects_only_descendants() {
        let bg = Ctx::background();
        let parent = bg.with_cancel();
        let child = parent.with_cancel();
        let sibling = parent.with_cancel();
        let grandchild = child.with_timeout(LONG);
        child.cancel();
        assert_eq!(child.err(), Some(CtxError::Canceled));
        assert_eq!(grandchild.err(), Some(CtxError::Canceled));
        wait_done(&grandchild).await;
        assert_eq!(parent.err(), None);
        assert_eq!(sibling.err(), None);
        assert!(not_done_within(&sibling, Duration::from_millis(5)).await);
        parent.cancel();
        assert_eq!(sibling.err(), Some(CtxError::Canceled));
    }

    #[tokio::test]
    async fn clones_share_one_context() {
        let c = Ctx::background().with_cancel();
        let c2 = c.clone();
        c2.cancel();
        assert_eq!(c.err(), Some(CtxError::Canceled));
        let bg = Ctx::background();
        let child = bg.clone().with_cancel();
        bg.cancel();
        assert_eq!(child.err(), Some(CtxError::Canceled));
    }

    #[tokio::test]
    async fn child_of_an_ended_parent_has_ended() {
        let parent = Ctx::background().with_cancel();
        parent.cancel();
        assert_eq!(parent.with_cancel().err(), Some(CtxError::Canceled));
        // The parent's error wins over the child's own zero timeout.
        assert_eq!(
            parent.with_timeout(Duration::ZERO).err(),
            Some(CtxError::Canceled)
        );
        assert_eq!(parent.with_timeout(LONG).err(), Some(CtxError::Canceled));
        wait_done(&parent.with_timeout(LONG)).await;

        let expired = Ctx::background().with_timeout(Duration::ZERO);
        assert_eq!(
            expired.with_cancel().err(),
            Some(CtxError::DeadlineExceeded)
        );
        assert_eq!(
            expired.with_timeout(LONG).err(),
            Some(CtxError::DeadlineExceeded)
        );
    }

    #[tokio::test]
    async fn zero_timeout_has_ended_at_creation() {
        let c = Ctx::background().with_timeout(Duration::ZERO);
        assert_eq!(c.err(), Some(CtxError::DeadlineExceeded));
        c.cancel();
        assert_eq!(c.err(), Some(CtxError::DeadlineExceeded));
        wait_done(&c).await;
        assert_eq!(c.run(async { 1 }).await, Err(CtxError::DeadlineExceeded));
    }

    #[tokio::test]
    async fn timeouts_nest() {
        let bg = Ctx::background();
        let parent = bg.with_timeout(Duration::from_secs(60));
        let Some(pd) = parent.deadline() else {
            panic!("with_timeout sets a deadline");
        };
        assert_eq!(parent.with_cancel().deadline(), Some(pd));
        assert_eq!(
            parent.with_timeout(LONG).deadline(),
            Some(pd),
            "a child never outlives its parent"
        );
        let Some(cd) = parent.with_timeout(Duration::from_secs(1)).deadline() else {
            panic!("with_timeout sets a deadline");
        };
        assert!(cd < pd);
        // A timeout beyond the clock's range keeps the parent's deadline.
        assert_eq!(parent.with_timeout(Duration::MAX).deadline(), Some(pd));
        assert_eq!(bg.with_timeout(Duration::MAX).deadline(), None);
    }

    #[tokio::test]
    async fn deadline_ends_the_ctx() {
        let start = Instant::now();
        let c = Ctx::background().with_timeout(SHORT);
        let early = c.err();
        // The deadline has not passed unless SHORT elapsed, e.g. while the test thread was stalled.
        if start.elapsed() < SHORT {
            assert_eq!(early, None);
        }
        wait_done(&c).await;
        assert_eq!(c.err(), Some(CtxError::DeadlineExceeded));
        // Cancelling after the deadline keeps the first cause.
        c.cancel();
        assert_eq!(c.err(), Some(CtxError::DeadlineExceeded));
        assert_eq!(c.with_cancel().err(), Some(CtxError::DeadlineExceeded));
        assert_eq!(
            c.run(std::future::pending::<()>()).await,
            Err(CtxError::DeadlineExceeded)
        );
    }

    #[tokio::test]
    async fn parent_deadline_reaches_children() {
        let parent = Ctx::background().with_timeout(SHORT);
        let child = parent.with_cancel();
        let longer = parent.with_timeout(LONG);
        wait_done(&child).await;
        assert_eq!(child.err(), Some(CtxError::DeadlineExceeded));
        wait_done(&longer).await;
        assert_eq!(longer.err(), Some(CtxError::DeadlineExceeded));
        assert_eq!(parent.err(), Some(CtxError::DeadlineExceeded));
    }

    /// `want` when the cancel provably came before the deadline (`in_time`); otherwise, e.g. with a
    /// test thread stalled for SHORT, the deadline may have come first and only an end is certain.
    fn assert_first_cause(got: Option<CtxError>, in_time: bool, want: CtxError) {
        if in_time {
            assert_eq!(got, Some(want));
        } else {
            assert!(got.is_some(), "the ctx has ended");
        }
    }

    #[tokio::test]
    async fn cancel_before_the_deadline_stays_canceled() {
        // Each deadline is at least SHORT after `start`, so a cancel observed before SHORT elapsed came
        // strictly before it.
        let start = Instant::now();
        let c = Ctx::background().with_timeout(SHORT);
        c.cancel();
        let in_time = start.elapsed() < SHORT;
        tokio::time::sleep(SHORT * 2).await;
        assert_first_cause(c.err(), in_time, CtxError::Canceled);

        // A child cancelled before its parent's deadline.
        let start = Instant::now();
        let parent = Ctx::background().with_timeout(SHORT);
        let child = parent.with_timeout(LONG);
        child.cancel();
        let in_time = start.elapsed() < SHORT;
        wait_done(&parent).await;
        assert_eq!(parent.err(), Some(CtxError::DeadlineExceeded));
        assert_first_cause(child.err(), in_time, CtxError::Canceled);

        // A parent cancelled before its child's deadline.
        let start = Instant::now();
        let parent = Ctx::background().with_cancel();
        let child = parent.with_timeout(SHORT);
        parent.cancel();
        let in_time = start.elapsed() < SHORT;
        tokio::time::sleep(SHORT * 2).await;
        assert_first_cause(child.err(), in_time, CtxError::Canceled);
    }

    #[tokio::test]
    async fn unobserved_deadline_wins_over_a_later_cancel() {
        let parent = Ctx::background().with_timeout(SHORT);
        let child = parent.with_cancel();
        // Nobody looks at either ctx until well after the deadline.
        std::thread::sleep(SHORT * 2);
        child.cancel();
        parent.cancel();
        assert_eq!(child.err(), Some(CtxError::DeadlineExceeded));
        assert_eq!(parent.err(), Some(CtxError::DeadlineExceeded));
    }

    #[tokio::test]
    async fn run_returns_the_ctx_error_when_the_ctx_ends_first() {
        let c = Ctx::background().with_cancel();
        let canceller = {
            let c = c.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(10)).await;
                c.cancel();
            })
        };
        assert_eq!(
            c.run(std::future::pending::<()>()).await,
            Err(CtxError::Canceled)
        );
        if let Err(e) = canceller.await {
            panic!("canceller failed: {e}");
        }
        let t = Ctx::background().with_timeout(SHORT);
        assert_eq!(
            t.run(std::future::pending::<()>()).await,
            Err(CtxError::DeadlineExceeded)
        );
    }

    #[tokio::test]
    async fn run_on_an_ended_ctx_does_not_poll() {
        let c = Ctx::background().with_cancel();
        c.cancel();
        let polled = AtomicBool::new(false);
        let r = c
            .run(async {
                polled.store(true, Ordering::SeqCst);
            })
            .await;
        assert_eq!(r, Err(CtxError::Canceled));
        assert!(!polled.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn sleep_honours_the_ctx() {
        let bg = Ctx::background();
        assert_eq!(bg.sleep(Duration::ZERO).await, Ok(()));
        assert_eq!(
            bg.with_timeout(SHORT).sleep(LONG).await,
            Err(CtxError::DeadlineExceeded)
        );
        let c = bg.with_cancel();
        c.cancel();
        assert_eq!(c.sleep(LONG).await, Err(CtxError::Canceled));
        assert_eq!(
            bg.with_timeout(LONG).sleep(Duration::from_millis(1)).await,
            Ok(())
        );
        // Durations beyond the clock's range do not overflow.
        let c = bg.with_timeout(SHORT);
        assert_eq!(
            c.sleep(Duration::MAX).await,
            Err(CtxError::DeadlineExceeded)
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn racing_observers_agree() {
        for _ in 0..20 {
            let parent = Ctx::background().with_timeout(Duration::from_millis(2));
            let child = parent.with_cancel();
            let mut tasks = Vec::new();
            for i in 0..8 {
                let parent = parent.clone();
                let child = child.clone();
                tasks.push(tokio::spawn(async move {
                    if i % 3 == 0 {
                        std::thread::sleep(Duration::from_millis(2));
                        parent.cancel();
                    }
                    child.done().await;
                    (parent.err(), child.err())
                }));
            }
            let mut seen = Vec::new();
            for t in tasks {
                match t.await {
                    Ok(v) => seen.push(v),
                    Err(e) => panic!("observer failed: {e}"),
                }
            }
            let (p, c) = (parent.err(), child.err());
            assert!(p.is_some());
            assert_eq!(p, c, "a child ends with its parent's error");
            for v in seen {
                assert_eq!(v, (p, c));
            }
        }
    }

    #[test]
    fn debug_shows_deadline_and_err() {
        let c = Ctx::background().with_cancel();
        c.cancel();
        let s = format!("{c:?}");
        assert!(s.contains("Canceled"), "{s}");
    }
}
