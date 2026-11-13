//! The log is in storage, and comes back a page at a time: to send it,
//! to settle it, and to show what turned out to be live.

use super::*;

/// More transactions than fit in a page, which is three in tests.
const MANY: usize = 10;

#[test]
fn a_long_backlog_reaches_everyone_and_settles() {
    let mut cluster = household();
    cluster.disconnect_everyone();
    cluster.settle();
    for index in 0..MANY {
        write(&mut cluster, PHONE, &format!("note/{index}"), "offline");
    }

    cluster.connect_everyone();
    cluster.settle();

    for device in [PHONE, LAPTOP, NAS, FRIEND] {
        for index in 0..MANY {
            let note = read_committed(&cluster, device, &format!("note/{index}"));
            assert_eq!(note.as_deref(), Some("offline"), "device {device}");
        }
    }

    for device in [PHONE, LAPTOP, NAS] {
        assert_eq!(stored_log_len(&cluster, device), 0, "device {device}");
        assert_eq!(stored_frontier(&cluster, device), [], "device {device}");
    }
}

/// Two long histories written apart interleave in the timeline, so each
/// side gets pages that land in the middle of what it has.
#[test]
fn two_long_backlogs_merge() {
    let mut cluster = household();
    cluster.disconnect_everyone();
    cluster.settle();
    for index in 0..MANY {
        write(&mut cluster, PHONE, "color", &format!("blue {index}"));
        write(&mut cluster, LAPTOP, "color", &format!("red {index}"));
        claim(&mut cluster, PHONE, &format!("name/{index}"), "phone");
        claim(&mut cluster, LAPTOP, &format!("name/{index}"), "laptop");
    }

    cluster.connect_everyone();
    cluster.settle();

    let color = read_committed(&cluster, PHONE, "color");
    assert!(color.is_some());
    for device in [LAPTOP, NAS, FRIEND] {
        assert_eq!(read_committed(&cluster, device, "color"), color);
        for index in 0..MANY {
            let name = format!("name/{index}");
            assert_eq!(
                holder(&cluster, device, &name),
                holder(&cluster, PHONE, &name),
                "device {device}"
            );
        }
    }

    assert_eq!(stored_log_len(&cluster, NAS), 0);
}

/// A claim's verdict isn't remembered across a restart. It's worked out
/// again, and what it wrote is read back from the log.
#[test]
fn an_unsettled_claim_is_still_held_after_a_restart() {
    let mut cluster = household();
    cluster.disconnect_everyone();
    cluster.settle();
    claim(&mut cluster, PHONE, "ada", "phone");
    cluster.settle();
    assert_eq!(holder(&cluster, PHONE, "ada").as_deref(), Some("phone"));

    cluster.restart(PHONE);
    assert_eq!(holder(&cluster, PHONE, "ada").as_deref(), Some("phone"));
    assert_eq!(read(&cluster, PHONE, "owner/ada").as_deref(), Some("phone"));

    cluster.connect_everyone();
    cluster.settle();
    assert_eq!(holder(&cluster, LAPTOP, "ada").as_deref(), Some("phone"));
    assert_eq!(stored_log_len(&cluster, PHONE), 0);
}

/// Starting up reads the log a few entries at a time, and keeps only
/// an outline of each. Everything is still there to send afterwards.
#[test]
fn a_long_unsettled_log_comes_back_after_a_restart() {
    let mut cluster = household();
    cluster.disconnect_everyone();
    cluster.settle();
    for index in 0..MANY {
        write(&mut cluster, PHONE, &format!("note/{index}"), "offline");
    }

    cluster.settle();
    let clock = group(&cluster, PHONE).log().clock().clone();
    cluster.restart(PHONE);
    assert_eq!(group(&cluster, PHONE).log().clock(), &clock);
    assert_eq!(stored_log_len(&cluster, PHONE), MANY);

    cluster.connect_everyone();
    cluster.settle();
    for index in 0..MANY {
        let note = read_committed(&cluster, LAPTOP, &format!("note/{index}"));
        assert_eq!(note.as_deref(), Some("offline"));
    }

    assert_eq!(stored_log_len(&cluster, PHONE), 0);
}
