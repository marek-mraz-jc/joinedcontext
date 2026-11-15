//! The consumer group protocol: the messages ekiben sends to a group coordinator, the consumer
//! protocol's subscription and assignment, and the range assignor the group leader runs.

use std::collections::{BTreeMap, BTreeSet};

use bytes::Bytes;

use crate::wire::{Dec, Enc, WireResult};

/// The group id an organization's input joins: its id and a dot before the configured name, so
/// one organization never joins, reads or commits another's group.
pub fn group_id(org: &str, consumer_group: &str) -> String {
    let prefix = format!("{org}.");
    if consumer_group.starts_with(&prefix) {
        consumer_group.to_string()
    } else {
        format!("{prefix}{consumer_group}")
    }
}

pub fn find_coordinator(group: &str) -> Vec<u8> {
    let mut e = Enc::default();
    e.str(group).i8(0);
    e.0
}

/// FindCoordinator v1: (error, node id, host, port).
pub fn decode_coordinator(mut d: Dec) -> WireResult<(i16, i32, String, i32)> {
    d.i32()?;
    let code = d.i16()?;
    d.null_str()?;
    Ok((code, d.i32()?, d.str()?, d.i32()?))
}

/// The consumer protocol's subscription (version 0).
pub fn subscription(topics: &[String]) -> Vec<u8> {
    let mut e = Enc::default();
    e.i16(0).array(topics, |e, t| {
        e.str(t);
    });
    e.i32(-1);
    e.0
}

pub fn decode_subscription(b: Bytes) -> WireResult<Vec<String>> {
    let mut d = Dec(b);
    d.i16()?;
    d.array(|d| d.str())
}

pub type Assignment = BTreeMap<String, Vec<i32>>;

pub fn assignment(a: &Assignment) -> Vec<u8> {
    let mut e = Enc::default();
    let items: Vec<(&String, &Vec<i32>)> = a.iter().collect();
    e.i16(0).array(&items, |e, (t, ps)| {
        e.str(t).array(ps, |e, p| {
            e.i32(*p);
        });
    });
    e.i32(-1);
    e.0
}

pub fn decode_assignment(b: Bytes) -> WireResult<Assignment> {
    if b.is_empty() {
        return Ok(Assignment::new());
    }
    let mut d = Dec(b);
    d.i16()?;
    Ok(d.array(|d| Ok((d.str()?, d.array(|d| d.i32())?)))?
        .into_iter()
        .collect())
}

pub fn join(
    group: &str,
    session_ms: i32,
    rebalance_ms: i32,
    member: &str,
    topics: &[String],
) -> Vec<u8> {
    let meta = subscription(topics);
    let mut e = Enc::default();
    e.str(group)
        .i32(session_ms)
        .i32(rebalance_ms)
        .str(member)
        .str("consumer")
        .array(&[()], |e, _| {
            e.str("range").bytes(&meta);
        });
    e.0
}

pub struct Joined {
    pub code: i16,
    pub generation: i32,
    pub protocol: String,
    pub leader: String,
    pub member: String,
    pub members: Vec<(String, Bytes)>,
}

pub fn decode_join(mut d: Dec) -> WireResult<Joined> {
    d.i32()?;
    Ok(Joined {
        code: d.i16()?,
        generation: d.i32()?,
        protocol: d.str()?,
        leader: d.str()?,
        member: d.str()?,
        members: d.array(|d| Ok((d.str()?, d.bytes()?)))?,
    })
}

pub fn sync(
    group: &str,
    generation: i32,
    member: &str,
    assignments: &[(String, Vec<u8>)],
) -> Vec<u8> {
    let mut e = Enc::default();
    e.str(group)
        .i32(generation)
        .str(member)
        .array(assignments, |e, (m, a)| {
            e.str(m).bytes(a);
        });
    e.0
}

pub fn decode_sync(mut d: Dec) -> WireResult<(i16, Bytes)> {
    d.i32()?;
    Ok((d.i16()?, d.bytes()?))
}

pub fn heartbeat(group: &str, generation: i32, member: &str) -> Vec<u8> {
    let mut e = Enc::default();
    e.str(group).i32(generation).str(member);
    e.0
}

/// Heartbeat v1 and LeaveGroup v1 answer a throttle time and an error code.
pub fn decode_code(mut d: Dec) -> WireResult<i16> {
    d.i32()?;
    d.i16()
}

pub fn leave(group: &str, member: &str) -> Vec<u8> {
    let mut e = Enc::default();
    e.str(group).str(member);
    e.0
}

pub fn offset_fetch(group: &str, a: &Assignment) -> Vec<u8> {
    let items: Vec<(&String, &Vec<i32>)> = a.iter().collect();
    let mut e = Enc::default();
    e.str(group).array(&items, |e, (t, ps)| {
        e.str(t).array(ps, |e, p| {
            e.i32(*p);
        });
    });
    e.0
}

/// OffsetFetch v1: (topic, partition, committed offset or -1, error).
pub fn decode_offset_fetch(mut d: Dec) -> WireResult<Vec<(String, i32, i64, i16)>> {
    let mut out = Vec::new();
    for (t, parts) in d.array(|d| {
        let t = d.str()?;
        let parts = d.array(|d| {
            let p = d.i32()?;
            let off = d.i64()?;
            d.null_str()?;
            Ok((p, off, d.i16()?))
        })?;
        Ok((t, parts))
    })? {
        for (p, off, code) in parts {
            out.push((t.clone(), p, off, code));
        }
    }
    Ok(out)
}

pub fn offset_commit(
    group: &str,
    generation: i32,
    member: &str,
    offsets: &[(String, i32, i64)],
) -> Vec<u8> {
    let mut by_topic: BTreeMap<&str, Vec<(i32, i64)>> = BTreeMap::new();
    for (t, p, o) in offsets {
        by_topic.entry(t).or_default().push((*p, *o));
    }
    let items: Vec<(&str, Vec<(i32, i64)>)> = by_topic.into_iter().collect();
    let mut e = Enc::default();
    e.str(group)
        .i32(generation)
        .str(member)
        .i64(-1)
        .array(&items, |e, (t, ps)| {
            e.str(t).array(ps, |e, (p, o)| {
                e.i32(*p).i64(*o).null_str();
            });
        });
    e.0
}

/// OffsetCommit v2: the first error a partition answered, or 0.
pub fn decode_offset_commit(mut d: Dec) -> WireResult<i16> {
    let mut first = 0;
    for codes in d.array(|d| {
        d.str()?;
        d.array(|d| {
            d.i32()?;
            d.i16()
        })
    })? {
        if let Some(c) = codes.into_iter().find(|c| *c != 0) {
            if first == 0 {
                first = c;
            }
        }
    }
    Ok(first)
}

/// The range assignor (Kafka's RangeAssignor): per topic, the members subscribed to it in id
/// order take consecutive partition ranges, the first ones one partition more.
pub fn range_assign(
    members: &[(String, Vec<String>)],
    partitions: &BTreeMap<String, usize>,
) -> BTreeMap<String, Assignment> {
    let mut out: BTreeMap<String, Assignment> = members
        .iter()
        .map(|(m, _)| (m.clone(), Assignment::new()))
        .collect();
    let topics: BTreeSet<&String> = members.iter().flat_map(|(_, ts)| ts).collect();
    for t in topics {
        let Some(&count) = partitions.get(t) else {
            continue;
        };
        let mut subs: Vec<&String> = members
            .iter()
            .filter(|(_, ts)| ts.contains(t))
            .map(|(m, _)| m)
            .collect();
        subs.sort();
        let (per, extra) = (count / subs.len(), count % subs.len());
        let mut next = 0;
        for (i, m) in subs.into_iter().enumerate() {
            let n = per + usize::from(i < extra);
            if n > 0 {
                let ps = (next..next + n).map(|p| p as i32).collect();
                out.entry(m.clone()).or_default().insert(t.clone(), ps);
            }
            next += n;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_group_id_carries_the_organization() {
        assert_eq!(group_id("org-a", "traffic"), "org-a.traffic");
        assert_eq!(group_id("org-a", "org-a.traffic"), "org-a.traffic");
        // another organization's prefix is just a name inside this one's namespace
        assert_eq!(group_id("org-a", "org-b.traffic"), "org-a.org-b.traffic");
    }

    #[test]
    fn range_assignment_matches_kafka() {
        let members = vec![
            ("m2".to_string(), vec!["a".to_string(), "b".to_string()]),
            ("m1".to_string(), vec!["a".to_string()]),
        ];
        let parts = BTreeMap::from([("a".to_string(), 5), ("b".to_string(), 1)]);
        let got = range_assign(&members, &parts);
        assert_eq!(
            got["m1"],
            Assignment::from([("a".to_string(), vec![0, 1, 2])])
        );
        assert_eq!(
            got["m2"],
            Assignment::from([("a".to_string(), vec![3, 4]), ("b".to_string(), vec![0])])
        );
    }

    #[test]
    fn subscription_and_assignment_round_trip() {
        let topics = vec!["x".to_string(), "y".to_string()];
        assert_eq!(
            decode_subscription(subscription(&topics).into()).expect("sub"),
            topics
        );
        let a = Assignment::from([("x".to_string(), vec![0, 2])]);
        assert_eq!(
            decode_assignment(assignment(&a).into()).expect("assignment"),
            a
        );
        assert!(decode_assignment(Bytes::new()).expect("empty").is_empty());
    }

    #[test]
    fn a_hostile_count_fails_without_reserving_memory() {
        let mut e = Enc::default();
        e.i16(0).i32(i32::MAX);
        assert!(decode_subscription(e.0.into()).is_err());
    }
}
