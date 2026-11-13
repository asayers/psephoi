use crate::fetch::{election, get_manifest, get_results};
use crate::{released_psephoi, retrieve_psephoi};
use anyhow::Result;
use gloo_utils::document;
use maud::{Markup, html};
use psephoi::{NONCE_LEN, RegWhileVoting, SIG_LEN, UnixSeconds, render_manifest};
use rand::rng;
use rand::seq::SliceRandom;
use web_sys::Element;

pub async fn render() -> Result<Markup> {
    let election = election().await?;
    let manifest = get_manifest().await?;
    let mut candidates = manifest
        .candidates
        .iter()
        .enumerate()
        .map(|(i, x)| (i, x.name()))
        .collect::<Vec<_>>();
    candidates.shuffle(&mut rng());

    let now = UnixSeconds::now();
    let voting = manifest.voting_period();
    let main = if now > voting.end {
        let results = get_results().await?;
        let total = results
            .candidates
            .iter()
            .map(|c| c.tally as usize)
            .sum::<usize>();
        // TODO: Highlight winner(s) in tally
        html! {
            fieldset #tally {
                legend { "Results" }
                p { "The election is finished and the results have been announced." }
                @for (i, c) in &candidates {
                    div {
                        input readonly id={"tally-" (i)} value=(results.candidates[*i].tally)
                        label for={"tally-" (i)} { (c) }
                        span id={"tally-status-" (i)};
                    }
                }
                div {
                    span {
                        "Verify that your vote is correctly counted.  Requires downloading "
                        (total as usize * NONCE_LEN / 1024 / 1024)
                        " MiB"
                    }
                    button #btn-Verify { "Verify" }
                }
                div {
                    span {
                        "Verify the validity of the entire set of votes.  Requires downloading "
                        (total as usize * (NONCE_LEN + SIG_LEN) / 1024 / 1024)
                        " MiB"
                    }
                    button #btn-Audit disabled { "Audit" }
                }
            }
        }
    } else if now > voting.start {
        let vote = released_psephoi().await?;
        let is_registered = retrieve_psephoi().await?.is_some();
        if !is_registered && manifest.reg_while_voting == RegWhileVoting::Disallow {
            html! {
                fieldset disabled #vote {
                    legend { "Vote" }
                    span { "The registration window has closed and you didn't \
                            register, so unfortunately you can't vote." }
                    @for (i, c) in &candidates {
                        div {
                            input disabled type="checkbox" id={"candidate-" (i)} name=(c)
                            label for={"candidate-" (i)} { (c) }
                        }
                    }
                    button #btn-Vote disabled { "Vote" }
                }
            }
        } else {
            let txt = if vote.is_empty() {
                "Ready to vote.  Vote for as many as you like."
            } else {
                "Already voted.  You can vote again, but note that you can only \
                expand your selection; once you vote for a candidate, you \
                can't take it back."
            };
            // TODO: Hide previous selection until they click a button
            html! {
                fieldset #vote {
                    legend { "Vote" }
                    span { (txt) }
                    @if !is_registered {
                        p .warning { strong {"Warning:"}
                        " You haven't registered yet.  This election allows late \
                        registration, but doing this compromises the secrecy \
                        of your vote." }
                    }
                    @for (i, c) in &candidates {
                        div {
                            input type="checkbox" id={"candidate-" (i)} name=(c) checked[vote.contains(c)] disabled[vote.contains(c)]
                            label for={"candidate-" (i)} { (c) }
                        }
                    }
                    button #btn-Vote { "Vote" }
                }
            }
        }
    } else if retrieve_psephoi().await?.is_some() {
        html! {
            fieldset #register {
                legend { "Register" }
                span { "Thanks for registering.  Voting will be " (voting.start) " to " (voting.end) }
                button #btn-Register disabled { "Register" }
            }
            fieldset disabled #vote {
                legend { "Vote" }
                @for (i, c) in &candidates {
                    div {
                        input disabled type="checkbox" id={"candidate-" (i)} name=(c)
                        label for={"candidate-" (i)} { (c) }
                    }
                }
                button #btn-Vote disabled { "Vote" }
            }
        }
    } else {
        html! {
            fieldset #register {
                legend { "Register" }
                span { "Ready to register" }
                button #btn-Register { "Register" }
            }
            fieldset disabled #vote {
                legend { "Vote" }
                @for (i, c) in &candidates {
                    div {
                        input disabled type="checkbox" id={"candidate-" (i)} name=(c)
                        label for={"candidate-" (i)} { (c) }
                    }
                }
                button #btn-Vote disabled { "Vote" }
            }
        }
    };

    let markup = html! {
        header { h1 {
            span .election_name { (manifest.name()) }
            br;
            span .emoji { (election.emoji()) }
        } }
        main {
            p #status-box { "" }
            (main)
            (render_manifest(manifest))
        }
    };
    Ok(markup)
}

pub fn status_box() -> Element {
    document().get_element_by_id("status-box").unwrap()
}
