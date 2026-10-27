//! Public piece state + run-length encoding (Go `PieceState` / `PieceStateRuns`).

use std::fmt;

use wt_types::{Completion, PiecePriority};

/// Go `PieceState` — value published by `/pieces` and `PieceStateRuns`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PiecePublicState {
    pub completion: Completion,
    pub priority: PiecePriority,
    /// Deprecated alias: `queued_for_hash || hashing`.
    pub checking: bool,
    pub hashing: bool,
    pub queued_for_hash: bool,
    pub marking: bool,
    pub partial: bool,
    pub missing_piece_layer_hash: bool,
}

/// Go `PieceStateRun` — consecutive pieces sharing the same state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PieceStateRun {
    pub state: PiecePublicState,
    pub length: u32,
}

/// Format one run exactly like Go `PieceStateRun.String`.
///
/// Examples: `3.`, `1!P`, `2HC`, `4?`, `1Nh`.
pub fn format_piece_state_run(run: &PieceStateRun) -> String {
    let mut ret = run.length.to_string();
    ret.push_str(match run.state.priority {
        PiecePriority::Next => "N",
        PiecePriority::Normal => ".",
        PiecePriority::Readahead => "R",
        PiecePriority::Now => "!",
        PiecePriority::High => "H",
        PiecePriority::None => "",
    });
    if run.state.hashing {
        ret.push('H');
    }
    if run.state.queued_for_hash {
        ret.push('Q');
    }
    if run.state.marking {
        ret.push('M');
    }
    if run.state.partial {
        ret.push('P');
    }
    if run.state.completion.ok && run.state.completion.complete {
        ret.push('C');
    }
    if !run.state.completion.ok {
        ret.push('?');
    }
    if run.state.missing_piece_layer_hash {
        ret.push('h');
    }
    ret
}

impl fmt::Display for PieceStateRun {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&format_piece_state_run(self))
    }
}

/// Run-length encode a sequence of piece states (Go `pieceStateRuns`).
pub fn encode_runs(states: impl IntoIterator<Item = PiecePublicState>) -> Vec<PieceStateRun> {
    let mut runs = Vec::new();
    let mut iter = states.into_iter();
    let Some(first) = iter.next() else {
        return runs;
    };
    let mut cur = first;
    let mut len = 1u32;
    for st in iter {
        if st == cur {
            len += 1;
        } else {
            runs.push(PieceStateRun {
                state: cur,
                length: len,
            });
            cur = st;
            len = 1;
        }
    }
    runs.push(PieceStateRun {
        state: cur,
        length: len,
    });
    runs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_string_format() {
        let run = PieceStateRun {
            state: PiecePublicState {
                completion: Completion {
                    ok: true,
                    complete: true,
                    err: None,
                },
                priority: PiecePriority::Normal,
                checking: false,
                hashing: false,
                queued_for_hash: false,
                marking: false,
                partial: false,
                missing_piece_layer_hash: false,
            },
            length: 3,
        };
        assert_eq!(format_piece_state_run(&run), "3.C");

        let run = PieceStateRun {
            state: PiecePublicState {
                completion: Completion::default(),
                priority: PiecePriority::Now,
                checking: true,
                hashing: false,
                queued_for_hash: true,
                marking: false,
                partial: true,
                missing_piece_layer_hash: false,
            },
            length: 1,
        };
        assert_eq!(format_piece_state_run(&run), "1!QP?");
    }

    #[test]
    fn rle_groups_equal_states() {
        let a = PiecePublicState {
            priority: PiecePriority::Normal,
            completion: Completion {
                ok: true,
                complete: false,
                err: None,
            },
            ..Default::default()
        };
        let b = PiecePublicState {
            priority: PiecePriority::Now,
            ..a.clone()
        };
        let runs = encode_runs([a.clone(), a.clone(), b.clone(), b]);
        assert_eq!(runs.len(), 2);
        assert_eq!(runs[0].length, 2);
        assert_eq!(runs[1].length, 2);
        assert_eq!(runs[0].state.priority, PiecePriority::Normal);
        assert_eq!(runs[1].state.priority, PiecePriority::Now);
    }
}
