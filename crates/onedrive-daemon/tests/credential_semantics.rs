//! Groundwork for the credential-state surface: what the pinned
//! hydration-graph `TokenCache` actually does, measured with a scripted
//! transport rather than asserted from its documentation.
//!
//! The auth-state socket (`src/auth_state.rs`) publishes one conclusion —
//! healthy / unsaved / rejected — and every conclusion rests on inferences
//! about the cache these tests hold still:
//!
//!  * In a daemon whose `resume()` succeeded at startup, `is_signed_in()`
//!    turning false can mean exactly one thing: the service refused the
//!    stored credential `MAX_REJECTIONS` times running. Nothing else clears
//!    it — not a transport outage, not a single `invalid_grant`, not a store
//!    failure. That is what lets the publisher translate `!is_signed_in()`
//!    as "sign-in required" instead of hedging.
//!  * The rejection count is *consecutive*: any non-`invalid_grant` failure
//!    between two `invalid_grant`s resets it. On a flaky link the death
//!    sentence can therefore be postponed indefinitely, so "rejected" is a
//!    state the daemon may reach minutes after the revocation, not seconds.
//!  * Once rejected, the cache stops spending the credential: no further
//!    token request leaves the process. A tray showing "sign-in required"
//!    is therefore describing a settled fact, not a retry in progress.
//!  * `last_store_error()` mirrors only the persist path — a rotation that
//!    could not be written back — and clears itself on the next save that
//!    works. Sync keeps working in the meantime; the cost is deferred to
//!    the next restart, which is exactly what the "unsaved" wording says.
//!  * `sign_in_with` (which `resume()` calls) lifts the death sentence.
//!    This is what makes adopt-by-restart work after a fresh enrollment —
//!    and also why a restart with the *same* dead credential buys a short
//!    "healthy" before the service rejects it again.
//!
//! No live credential and no network: the repository rule, and also the
//! point — these are the semantics everything else is built on, so they
//! must be checkable on any machine.

use hydration_graph::auth::{
    AuthConfig, AuthError, Clock, CredentialStore, RefreshToken, TokenCache, TokenReply,
    TokenRequest, TokenTransport,
};
use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One scripted answer per refresh POST, plus a count of how many were spent.
/// The count is the load-bearing assertion: "no request after rejection" is
/// only measurable as a number that stops moving.
struct Script {
    replies: Mutex<VecDeque<io::Result<TokenReply>>>,
    posts: AtomicU64,
}

impl Script {
    fn new(replies: impl IntoIterator<Item = io::Result<TokenReply>>) -> Self {
        Self {
            replies: Mutex::new(replies.into_iter().collect()),
            posts: AtomicU64::new(0),
        }
    }

    fn posts(&self) -> u64 {
        self.posts.load(Ordering::SeqCst)
    }
}

impl TokenTransport for Script {
    fn post(&self, _request: &TokenRequest) -> io::Result<TokenReply> {
        self.posts.fetch_add(1, Ordering::SeqCst);
        self.replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("the script covers every request the test provokes")
    }
}

fn invalid_grant() -> io::Result<TokenReply> {
    Ok(TokenReply::new(400, r#"{"error":"invalid_grant"}"#))
}

fn outage() -> io::Result<TokenReply> {
    Err(io::Error::new(io::ErrorKind::TimedOut, "no route"))
}

fn rotation(n: u32) -> io::Result<TokenReply> {
    Ok(TokenReply::new(
        200,
        format!(
            r#"{{"access_token":"access-{n}","expires_in":3600,"refresh_token":"refresh-{n}"}}"#
        ),
    ))
}

/// Manual time. `advance` walks the cache past its refresh backoff windows,
/// so the tests assert on attempts rather than living through the delays.
#[derive(Default)]
struct TestClock(Mutex<Duration>);

impl TestClock {
    fn advance(&self, by: Duration) {
        *self.0.lock().unwrap() += by;
    }
}

impl Clock for TestClock {
    fn now(&self) -> Duration {
        *self.0.lock().unwrap()
    }
    fn sleep(&self, how_long: Duration) {
        self.advance(how_long);
    }
}

/// An in-memory store whose save path can be broken and repaired, because
/// `last_store_error` is defined by exactly that transition.
#[derive(Default)]
struct Store {
    value: Mutex<Option<String>>,
    fail_save: Mutex<bool>,
}

impl Store {
    fn preloaded(value: &str) -> Self {
        Self {
            value: Mutex::new(Some(value.to_owned())),
            fail_save: Mutex::new(false),
        }
    }

    fn set_fail_save(&self, fail: bool) {
        *self.fail_save.lock().unwrap() = fail;
    }
}

impl CredentialStore for Store {
    fn load(&self) -> io::Result<Option<RefreshToken>> {
        Ok(self.value.lock().unwrap().clone().map(RefreshToken::new))
    }

    fn save(&self, refresh: &RefreshToken) -> io::Result<()> {
        if *self.fail_save.lock().unwrap() {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "the collection is locked",
            ));
        }
        *self.value.lock().unwrap() = Some(refresh.expose_for_storage().to_owned());
        Ok(())
    }
}

// The seams are shared by `Arc`, the same shape `SharedTokenCache` uses —
// the trait impls for `Arc<T>` are what production relies on too.
type Cache = TokenCache<Arc<Script>, Arc<TestClock>, Arc<Store>>;

fn cache(script: &Arc<Script>, clock: &Arc<TestClock>, store: &Arc<Store>) -> Cache {
    TokenCache::new(
        AuthConfig::public_client("test-client"),
        Arc::clone(script),
        Arc::clone(clock),
        Arc::clone(store),
    )
}

/// Long enough to clear any refresh backoff the tests accumulate (the cap is
/// five minutes), short enough not to expire a fresh access token.
const PAST_BACKOFF: Duration = Duration::from_secs(360);

#[test]
fn resume_answers_signed_in_without_spending_a_request() {
    let script = Arc::new(Script::new([]));
    let clock = Arc::new(TestClock::default());
    let store = Arc::new(Store::preloaded("refresh-0"));
    let cache = cache(&script, &clock, &store);

    assert!(cache.resume().unwrap());
    assert!(cache.is_signed_in());
    assert_eq!(script.posts(), 0, "resume is a load, not a refresh");

    // And the other answer: an empty store is "signed out", not an error.
    let empty = Arc::new(Store::default());
    let none = Arc::new(Script::new([]));
    let signed_out = TokenCache::new(
        AuthConfig::public_client("test-client"),
        Arc::clone(&none),
        Arc::clone(&clock),
        Arc::clone(&empty),
    );
    assert!(!signed_out.resume().unwrap());
    assert!(!signed_out.is_signed_in());
}

#[test]
fn three_consecutive_invalid_grants_flip_signed_in_and_stop_the_requests() {
    let script = Arc::new(Script::new([
        invalid_grant(),
        invalid_grant(),
        invalid_grant(),
    ]));
    let clock = Arc::new(TestClock::default());
    let store = Arc::new(Store::preloaded("refresh-0"));
    let cache = cache(&script, &clock, &store);
    assert!(cache.resume().unwrap());

    // One refusal is a blip, two are a coincidence: the cache stays signed
    // in through both, which is what keeps a service outage from reading as
    // a revocation in the tray.
    assert_eq!(cache.token().unwrap_err(), AuthError::InvalidGrant);
    assert!(
        cache.is_signed_in(),
        "one invalid_grant is not a conclusion"
    );
    clock.advance(PAST_BACKOFF);
    assert_eq!(cache.token().unwrap_err(), AuthError::InvalidGrant);
    assert!(
        cache.is_signed_in(),
        "two invalid_grants are not a conclusion"
    );
    clock.advance(PAST_BACKOFF);

    // The third consecutive one is the conclusion (MAX_REJECTIONS = 3).
    assert_eq!(cache.token().unwrap_err(), AuthError::InvalidGrant);
    assert!(
        !cache.is_signed_in(),
        "three consecutive invalid_grants are the death sentence"
    );

    // From here the cache reports the conclusion without presenting the
    // retired credential to the service again — the request count is the
    // proof, and it is why "sign-in required" describes a settled state.
    clock.advance(PAST_BACKOFF);
    assert_eq!(cache.token().unwrap_err(), AuthError::CredentialRejected);
    assert_eq!(script.posts(), 3, "a rejected credential is never re-spent");
}

#[test]
fn an_outage_between_invalid_grants_resets_the_death_count() {
    // invalid_grant, outage, invalid_grant, outage, invalid_grant: five
    // failures, three of them refusals, never three in a row.
    let script = Arc::new(Script::new([
        invalid_grant(),
        outage(),
        invalid_grant(),
        outage(),
        invalid_grant(),
    ]));
    let clock = Arc::new(TestClock::default());
    let store = Arc::new(Store::preloaded("refresh-0"));
    let cache = cache(&script, &clock, &store);
    assert!(cache.resume().unwrap());

    for _ in 0..5 {
        let _ = cache.token().unwrap_err();
        clock.advance(PAST_BACKOFF);
        assert!(
            cache.is_signed_in(),
            "non-consecutive refusals must not sign the user out"
        );
    }
    assert_eq!(script.posts(), 5);
    // The consequence worth knowing when reading the tray: on a flaky link,
    // "rejected" can arrive long after the revocation. The surface reports
    // the cache's conclusion, not the service's first refusal.
}

#[test]
fn a_store_that_cannot_persist_the_rotation_is_reported_and_recovers() {
    let script = Arc::new(Script::new([rotation(1), rotation(2)]));
    let clock = Arc::new(TestClock::default());
    let store = Arc::new(Store::preloaded("refresh-0"));
    let cache = cache(&script, &clock, &store);
    assert!(cache.resume().unwrap());

    // The refresh succeeds and sync continues; only the write-back failed.
    store.set_fail_save(true);
    cache.token().unwrap();
    assert!(
        cache.is_signed_in(),
        "an unsaved rotation is not a sign-out"
    );
    assert_eq!(
        cache.last_store_error(),
        Some(io::ErrorKind::PermissionDenied),
        "the persist failure must be readable, or the next restart's \
         sign-out would arrive with no warning at all"
    );

    // Expire the access token so the next ask refreshes again, this time
    // with the store repaired: the mirror must clear.
    store.set_fail_save(false);
    clock.advance(Duration::from_secs(4000));
    cache.token().unwrap();
    assert_eq!(cache.last_store_error(), None);
    assert_eq!(
        store.value.lock().unwrap().as_deref(),
        Some("refresh-2"),
        "the repaired save wrote the newest rotation"
    );
}

#[test]
fn sign_in_with_lifts_the_death_sentence() {
    let script = Arc::new(Script::new([
        invalid_grant(),
        invalid_grant(),
        invalid_grant(),
        rotation(1),
    ]));
    let clock = Arc::new(TestClock::default());
    let store = Arc::new(Store::preloaded("refresh-dead"));
    let cache = cache(&script, &clock, &store);
    assert!(cache.resume().unwrap());

    for _ in 0..3 {
        let _ = cache.token().unwrap_err();
        clock.advance(PAST_BACKOFF);
    }
    assert!(!cache.is_signed_in());

    // Different bytes, clean slate: this is the path a fresh enrollment
    // takes (resume() calls this), and it is why adopting an enrollment by
    // restarting the daemon works at all.
    cache.sign_in_with(RefreshToken::new("refresh-fresh"));
    assert!(cache.is_signed_in());
    cache.token().unwrap();
    assert_eq!(script.posts(), 4);
}
