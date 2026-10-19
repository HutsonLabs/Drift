//! HOST-15 Red: an advert's optional account and approval keys (ADR `HOST-15-guided-setup`).
//! Older hosts send neither: no account to fill in, and pairing needs the code.
#![allow(missing_docs, clippy::unwrap_used, clippy::expect_used)]

use drift_core::pairing::HostAdvert;
use uuid::Uuid;

fn txt(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
}

#[test]
fn account_and_approval_are_optional() {
    let id = Uuid::from_u128(1).to_string();
    let fp = "ab".repeat(32);
    let base = [("v", "1"), ("id", id.as_str()), ("host", "h.local"), ("fp", fp.as_str())];

    let old = HostAdvert::from_txt("Old", &txt(&base)).unwrap();
    assert_eq!(old.account, None);
    assert!(!old.approval);

    let mut new = base.to_vec();
    new.extend([("user", "andrew"), ("approve", "1")]);
    let new = HostAdvert::from_txt("New", &txt(&new)).unwrap();
    assert_eq!(new.account.as_deref(), Some("andrew"));
    assert!(new.approval);

    let mut odd = base.to_vec();
    odd.extend([("user", "  "), ("approve", "0")]);
    let odd = HostAdvert::from_txt("Odd", &txt(&odd)).unwrap();
    assert_eq!(odd.account, None, "a blank account is no account");
    assert!(!odd.approval);
}
