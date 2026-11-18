//! Each decision checked exhaustively on small inputs: every subset of 4
//! columns as the read set (and `Unknown`), as the changed columns and as the
//! copied fields, every event and every combination of the facts of [`Env`],
//! against the safety condition stated independently of the code: a TVIEW
//! depends on the table only through some set of columns `V`, which is any
//! subset of the stored read set when every fact holds, and any set at all
//! otherwise. A decision is safe when it holds for every such `V`.

use super::*;

const COLUMNS: i16 = 4;

/// Whether every attnum of `a` is in `b`.
fn is_subset(a: &Attnums, b: &Attnums) -> bool {
    a.iter().all(|c| b.contains(c))
}

/// Every subset of columns 1..=4.
fn subsets() -> Vec<Attnums> {
    (0_u8..16)
        .map(|mask| {
            (1..=COLUMNS)
                .filter(|c| mask & (1 << (c - 1)) != 0)
                .collect()
        })
        .collect()
}

fn read_sets() -> Vec<ReadSet> {
    let mut sets: Vec<ReadSet> = subsets().into_iter().map(ReadSet::Complete).collect();
    sets.push(ReadSet::Unknown);
    sets
}

const EVENTS: [Event; 3] = [Event::Insert, Event::Update, Event::Delete];

fn envs() -> Vec<Env> {
    (0_u8..16)
        .map(|m| Env {
            plan_current: m & 1 != 0,
            chain_reads_named: m & 2 != 0,
            owner_reads_all: m & 4 != 0,
            direct_patch_enabled: m & 8 != 0,
        })
        .collect()
}

/// The sets of columns the TVIEW may really depend on, given what the inputs
/// establish.
fn possible_reads(read: &ReadSet, env: Env) -> Vec<Attnums> {
    match read {
        ReadSet::Complete(r)
            if env.plan_current && env.chain_reads_named && env.owner_reads_all =>
        {
            subsets().into_iter().filter(|v| is_subset(v, r)).collect()
        }
        _ => subsets(),
    }
}

/// A write that leaves a TVIEW depending on columns `v` unchanged.
fn harmless(event: Event, changed: &Attnums, v: &Attnums) -> bool {
    event == Event::Update && changed.is_disjoint(v)
}

#[test]
fn column_skip_skips_only_what_no_possible_read_sees() {
    for read in read_sets() {
        for env in envs() {
            for event in EVENTS {
                for changed in subsets() {
                    let skip = column_skip(event, &read, &changed, env);
                    let safe = possible_reads(&read, env)
                        .iter()
                        .all(|v| harmless(event, &changed, v));
                    assert!(!skip || safe, "{read:?} {env:?} {event:?} {changed:?}");
                    // No lost skip: a trusted read set none of whose columns
                    // changed is skipped.
                    let trusted = env.plan_current && env.chain_reads_named && env.owner_reads_all;
                    if safe && trusted && matches!(read, ReadSet::Complete(_)) {
                        assert!(skip, "lost skip: {read:?} {env:?} {event:?} {changed:?}");
                    }
                }
            }
        }
    }
}

#[test]
fn statement_skip_is_the_column_skip_without_a_virtual_column() {
    for read in read_sets() {
        for env in envs() {
            for event in EVENTS {
                for changed in subsets() {
                    let row = column_skip(event, &read, &changed, env);
                    assert_eq!(statement_skip(event, &read, &changed, false, env), row);
                    assert!(!statement_skip(event, &read, &changed, true, env));
                }
            }
        }
    }
}

#[test]
fn pair_filter_compares_every_column_any_possible_read_sees() {
    for read in read_sets() {
        for env in envs() {
            for event in EVENTS {
                let Some(compared) = pair_filter(event, &read, env) else {
                    continue;
                };
                assert_eq!(event, Event::Update);
                // A pair equal on `compared` changed no column of any possible
                // read: dropping it is a skip.
                for v in possible_reads(&read, env) {
                    assert!(is_subset(&v, compared), "{read:?} {env:?} {v:?}");
                }
            }
        }
    }
}

/// Every assignment of the 4 columns to: not copied, copied exactly, copied
/// inexactly.
fn copies() -> Vec<Vec<Copied>> {
    (0..81_u32)
        .map(|mut m| {
            let mut out = Vec::new();
            for attnum in 1..=COLUMNS {
                match m % 3 {
                    1 => out.push(Copied {
                        attnum,
                        exact: true,
                    }),
                    2 => out.push(Copied {
                        attnum,
                        exact: false,
                    }),
                    _ => {}
                }
                m /= 3;
            }
            out
        })
        .collect()
}

fn exactly_copied(copied: &[Copied], attnum: i16) -> bool {
    copied.iter().any(|c| c.attnum == attnum && c.exact)
}

#[test]
fn direct_patch_copies_every_changed_column_any_possible_read_sees() {
    for read in read_sets() {
        for env in envs() {
            for event in EVENTS {
                for changed in subsets() {
                    for key_changed in [false, true] {
                        for copied in copies() {
                            let Some(fields) =
                                direct_patch(event, &read, &changed, key_changed, &copied, env)
                            else {
                                continue;
                            };
                            assert!(event == Event::Update && !key_changed);
                            assert!(env.direct_patch_enabled);
                            assert!(!fields.is_empty());
                            assert!(is_subset(&fields, &changed));
                            for v in possible_reads(&read, env) {
                                // Every changed column the TVIEW may read is
                                // written, by an exact copy.
                                for c in changed.intersection(&v).iter() {
                                    assert!(fields.contains(c), "{read:?} {env:?} {changed:?}");
                                    assert!(exactly_copied(&copied, c));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn fanout_patch_writes_only_exact_copies_of_what_any_possible_read_sees() {
    for read in read_sets() {
        for env in envs() {
            for event in EVENTS {
                for changed in subsets() {
                    for (key_changed, paired) in [(false, true), (true, true), (false, false)] {
                        for copied in copies() {
                            if !fanout_patch(
                                event,
                                &read,
                                &changed,
                                key_changed,
                                paired,
                                &copied,
                                env,
                            ) {
                                continue;
                            }
                            assert!(event == Event::Update && !key_changed && paired);
                            assert!(env.direct_patch_enabled);
                            for v in possible_reads(&read, env) {
                                for c in changed.intersection(&v).iter() {
                                    assert!(exactly_copied(&copied, c), "{read:?} {env:?}");
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn the_guards_and_the_prune_need_their_positive_fact() {
    assert_eq!(noop_guard(), Equality::Binary);
    for edge in [false, true] {
        for changed in [false, true] {
            assert_eq!(edge_prune(edge, changed), edge && !changed);
        }
    }
    for full in [false, true] {
        for guarded in [false, true] {
            for met in [false, true] {
                let all = lookup_filter(full, guarded, met);
                assert!(!all || full);
                // An unguarded table, or a guard met, refreshes the whole TVIEW.
                if full && (!guarded || met) {
                    assert!(all);
                }
            }
        }
    }
}

#[test]
fn attnums_behave_as_a_set() {
    let all = subsets();
    for a in &all {
        for b in &all {
            let i = a.intersection(b);
            for c in 1..=COLUMNS {
                assert_eq!(i.contains(c), a.contains(c) && b.contains(c));
            }
            assert_eq!(a.is_disjoint(b), i.is_empty());
        }
    }
    let wide: Attnums = [1, 64, 65, 1600, 0, -3].into_iter().collect();
    assert_eq!(wide.iter().collect::<Vec<_>>(), vec![1, 64, 65, 1600]);
    assert_eq!(wide.intersection(&Attnums::new()), Attnums::new());
}
