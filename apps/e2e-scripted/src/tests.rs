use super::*;
use stt_api::SttSessionConfig;

fn design() -> Scenario {
    Scenario::load(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("scenarios/design")
            .as_path(),
    )
    .unwrap()
}

#[test]
fn absent_environment_disables_override() {
    assert!(Scenario::load_optional(None).unwrap().is_none());
}

fn script() -> Scenario {
    let rows = [
        (Track::Remote, 100),
        (Track::SelfMic, 110),
        (Track::Remote, 120),
        (Track::SelfMic, 400),
    ]
    .into_iter()
    .map(|(track, end_ms)| Utterance {
        track,
        start_ms: end_ms - 10,
        end_ms,
        text: "synthetic utterance".into(),
    })
    .collect::<Vec<_>>();
    Scenario {
        name: "test".into(),
        settings: Settings::default(),
        rows: rows.into(),
    }
}

#[tokio::test(start_paused = true)]
async fn tracks_share_a_clock_and_preserve_script_order() {
    let (self_track, remote_track) = script().providers();
    let (_self_session, mut own) = self_track
        .start_session(SttSessionConfig::new(48_000))
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_millis(5)).await;
    let (_remote_session, mut remote) = remote_track
        .start_session(SttSessionConfig::new(48_000))
        .await
        .unwrap();
    tokio::select! {
        biased;
        _ = own.recv() => panic!("self track emitted before the earlier remote row"),
        event = remote.recv() => assert_final(event, 9, 10),
    }
    assert_final(own.recv().await, 10, 11);
    assert_final(remote.recv().await, 11, 12);
}

#[tokio::test(start_paused = true)]
async fn reconnect_resumes_and_finalize_closes_without_flushing() {
    let (self_track, remote_track) = script().providers();
    let (own_session, mut own) = self_track
        .start_session(SttSessionConfig::new(48_000))
        .await
        .unwrap();
    let (remote_session, mut remote) = remote_track
        .start_session(SttSessionConfig::new(48_000))
        .await
        .unwrap();
    assert_final(remote.recv().await, 9, 10);
    drop(remote);
    drop(remote_session);
    assert_final(own.recv().await, 10, 11);
    let (remote_session, mut remote) = remote_track
        .start_session(SttSessionConfig::new(48_000))
        .await
        .unwrap();
    assert_final(remote.recv().await, 11, 12);
    remote_session.finalize().await.unwrap();
    own_session.finalize().await.unwrap();
    assert!(remote.recv().await.is_none());
    assert!(own.recv().await.is_none());
}

#[test]
fn bundled_design_loads() {
    let scenario = design();
    assert_eq!(scenario.name(), "design");
    assert_eq!(scenario.settings.speed, 10);
    assert_eq!(scenario.settings.latency_ms, 300);
    assert_eq!(scenario.rows.len(), 150);
}

fn assert_final(event: Option<SttEvent>, start: u64, end: u64) {
    assert!(matches!(event, Some(SttEvent::FinalTranscript {
        words: None,
        audio_start_ms: Some(actual_start),
        audio_end_ms: Some(actual_end),
        ..
    }) if actual_start == start && actual_end == end));
}
