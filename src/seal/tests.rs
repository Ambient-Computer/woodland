use super::secret::{Grant, Secret};
use super::*;
use crate::testing::peers::{identity_of, laptop, phone};

fn secret() -> Secret {
    Secret::make(&identity_of(phone()), b"a group")
}

#[test]
fn a_device_makes_the_same_secret_for_the_same_purpose() {
    let phone = identity_of(phone());

    assert_eq!(Secret::make(&phone, b"a"), Secret::make(&phone, b"a"));
    assert_ne!(Secret::make(&phone, b"a"), Secret::make(&phone, b"b"));
    assert_ne!(
        Secret::make(&phone, b"a"),
        Secret::make(&identity_of(laptop()), b"a")
    );
}

#[test]
fn what_was_sealed_opens_under_the_same_secret_about_the_same_thing() {
    let sealed = secret().seal(b"entry 1", b"hello");

    assert_eq!(secret().open(b"entry 1", &sealed), Some(b"hello".to_vec()));
    assert_eq!(secret().open(b"entry 2", &sealed), None);
    let other = Secret::make(&identity_of(phone()), b"another group");
    assert_eq!(other.open(b"entry 1", &sealed), None);
}

#[test]
fn what_was_sealed_doesnt_show_through() {
    let plain = b"a secret worth keeping, long enough to find";
    let sealed = secret().seal(b"entry", plain);

    assert!(
        !sealed
            .windows(8)
            .any(|it| plain.windows(8).any(|p| p == it))
    );
    assert!(sealed.len() > plain.len());
}

#[test]
fn anything_changed_in_transit_doesnt_open() {
    let sealed = secret().seal(b"entry", b"hello");

    for index in 0..sealed.len() {
        let mut changed = sealed.clone();
        changed[index] ^= 1;
        assert_eq!(secret().open(b"entry", &changed), None, "byte {index}");
    }
    assert_eq!(secret().open(b"entry", &sealed[..10]), None);
    assert_eq!(secret().open(b"entry", &[]), None);
}

#[test]
fn different_contents_never_share_a_nonce() {
    let one = secret().seal(b"entry", b"hello");
    let other = secret().seal(b"entry", b"jello");
    let elsewhere = secret().seal(b"another entry", b"hello");

    assert_ne!(one[..24], other[..24]);
    assert_ne!(one[..24], elsewhere[..24]);
    assert_eq!(one, secret().seal(b"entry", b"hello"));
}

#[test]
fn a_grant_opens_for_the_peer_it_was_made_for() {
    let (phone, laptop) = (identity_of(phone()), identity_of(laptop()));
    let grant = secret().grant(&phone, &SealingKey::of(&laptop), b"height 3");

    assert_eq!(grant.open(&laptop, b"height 3"), Some(secret()));
    assert_eq!(grant.open(&laptop, b"height 4"), None);
    assert_eq!(grant.open(&phone, b"height 3"), None);
    assert!(!encoding_of(&grant).windows(32).any(|it| it == secret().0));
}

fn encoding_of(grant: &Grant) -> Vec<u8> {
    crate::encoding::encode(grant)
}

#[test]
fn a_sealing_key_is_vouched_for_by_its_owner_alone() {
    let key = SealingKey::of(&identity_of(phone()));

    assert!(key.is_vouched_by(phone()));
    assert!(!key.is_vouched_by(laptop()));

    // Someone else's key under this owner's name.
    let mut swapped = key.clone();
    swapped.key = SealingKey::of(&identity_of(laptop())).key;
    assert!(!swapped.is_vouched_by(phone()));
    let mut unsigned = key;
    unsigned.vouch.clear();
    assert!(!unsigned.is_vouched_by(phone()));
}

#[test]
fn a_secret_is_never_printed() {
    let printed = format!("{:?}", secret());

    assert!(!printed.contains(&format!("{:?}", secret().0)));
    assert_eq!(secret().commitment(), secret().commitment());
}
