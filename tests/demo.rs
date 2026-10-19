use anyhow::Result;
use cfvirtual::{
    domain::review,
    store::Store,
    tui::{demo_contest, run_demo_headless},
};
use std::time::Duration;

#[tokio::test]
async fn offline_demo_completes_on_a_logical_clock_and_survives_restart() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("demo.db");
    let before = tokio::time::Instant::now();
    // One second bounds an accidental real 90-second countdown/poll loop. No API
    // client is constructed anywhere on this entry point's call path.
    let (contest, standings) =
        tokio::time::timeout(Duration::from_secs(1), run_demo_headless(42, &path)).await??;
    assert!(before.elapsed() < Duration::from_secs(1));
    assert_eq!(contest.problems.len(), 6);
    assert_eq!(contest.bots.len(), 3);
    assert_eq!(contest.duration_sec, 90);
    assert_eq!(contest.ended_at, Some(contest.started_at + 90));
    assert_eq!(contest.host_handle, "demo_player");
    assert_eq!(standings.len(), 4);
    let human = standings.iter().find(|s| !s.is_bot).expect("demo human");
    assert_eq!(human.name, "demo_player");
    assert_eq!(human.solved, 3);
    assert_eq!(human.problems[&'D'].rejected, 1);
    assert_eq!(human.problems[&'E'].attempts, 0);
    assert_eq!(human.penalty, 18 + 44 + 63 + 2400);
    assert!(standings.iter().filter(|s| s.is_bot).any(|s| s.solved > 0));
    let mut restarted = Store::open(&path)?;
    let (saved, scores) = restarted.load_contest(&contest.id)?.expect("saved demo");
    assert_eq!(scores, standings);
    assert_eq!(saved.ended_at, contest.ended_at);
    assert_eq!(restarted.history(10)?.len(), 1);
    assert_eq!(
        restarted.enqueue_failed(&saved, human, contest.deadline())?,
        3
    );
    assert_eq!(restarted.review(false, 10, contest.deadline())?.len(), 3);
    let exported = review::markdown(&saved, &scores, "demo_player");
    assert!(exported.contains("(bot)"));
    assert!(exported.contains("## Links"));
    assert!(exported.contains("Solved: 3/6"));
    assert!(!exported.contains("participant_token"));
    assert!(saved.participants.iter().all(|p| p.token.is_empty()));
    Ok(())
}

#[tokio::test]
async fn seed_replays_the_same_fixture_and_final_standings() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("replay.db");
    let first = demo_contest(9);
    let second = demo_contest(9);
    assert_eq!(serde_json::to_value(first)?, serde_json::to_value(second)?);
    let (first, first_scores) = run_demo_headless(9, &path).await?;
    let (second, second_scores) = run_demo_headless(9, &path).await?;
    assert_eq!(first.id, second.id);
    assert_eq!(first_scores, second_scores);
    assert_eq!(Store::open(&path)?.history(10)?.len(), 1);
    Ok(())
}
