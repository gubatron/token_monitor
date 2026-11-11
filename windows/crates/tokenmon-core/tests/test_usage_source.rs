//! Port of Swift `UsageSourceTests`: the "This PC" wording, which every
//! local-figure label takes from `UsageSource` (issue #85).

#![allow(clippy::unwrap_used)]

use tokenmon_core::usage::UsageSource;

/// Raw values round-trip (string and serde forms) and unknown input is
/// rejected with a typed error.
#[test]
fn test_raw_values_round_trip_and_reject_unknown_input() {
    for source in UsageSource::ALL {
        assert_eq!(source.as_str().parse::<UsageSource>(), Ok(source));
        assert_eq!(source.to_string(), source.as_str());
        let json = serde_json::to_string(&source).unwrap();
        assert_eq!(json, format!("\"{}\"", source.as_str()));
        assert_eq!(serde_json::from_str::<UsageSource>(&json).unwrap(), source);
    }
    assert_eq!(UsageSource::Account.as_str(), "account");
    assert_eq!(UsageSource::ThisComputer.as_str(), "thisComputer");
    let err = "thisPC".parse::<UsageSource>().unwrap_err();
    assert_eq!(err.0, "thisPC");
    assert!(serde_json::from_str::<UsageSource>("\"thisPC\"").is_err());
}

/// Figures read on this computer say "This PC" everywhere.
#[test]
fn test_local_figures_say_this_pc() {
    let local = UsageSource::ThisComputer;
    assert_eq!(local.tag().as_deref(), Some("This PC"));
    assert_eq!(local.location_phrase().as_deref(), Some("on this PC"));
    assert_eq!(local.tokens_unit(), "tokens on this PC");
    assert_eq!(
        local.qualifying("Tokens this month"),
        "Tokens this month on this PC"
    );
}

/// Account-wide figures carry no label at all.
#[test]
fn test_account_figures_are_not_labelled() {
    let account = UsageSource::Account;
    assert_eq!(account.tag(), None);
    assert_eq!(account.location_phrase(), None);
    assert_eq!(account.tokens_unit(), "tokens");
    assert_eq!(account.qualifying("Monthly spend"), "Monthly spend");
}
