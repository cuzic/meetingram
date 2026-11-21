# ADR 0002 r3 のレビュー（round 3）

対象: `docs/adr/0002-e2e-scripted-backends.md` r3（12e4ff3）。round 2 との差分だけを確認しました。

**判定: 収束。** Blocker も Major もありません。

## round 2 の指摘の反映

| 指摘 | 状況 | 確認したこと |
| --- | --- | --- |
| M-1 | 正しく反映 | 次の 3 点を確認した。<br>・エッジを返すのは、端点の少なくとも一方が今回の `add_node` のときだけ。<br>・両端とも `context` にあるエッジは返さない。<br>・`grounds` は今回作る端点の最初の根拠なので、必ず今回の `new` にある。<br>この規則は、`GroundOutsideInput`（`patch.rs:269-273`）にも `EdgeDuplicate`（`patch.rs:339-346`）にも当たらない。round 2 のシミュレーションと同じ規則で、正解の 3 本とも全エッジが出る（区切りは 1〜12 件のどれでも）。 |
| m-a | 正しく反映 | `LiveTranscriptionOptions` は常に定義し、cfg を付けるのは `stt_override` と `with_stt_override` だけ。`windows_session.rs:69-85` と stub（`live_transcription.rs:2827-2840`）の両方でコンパイルできる。§2 L94 とも矛盾しない。 |
| m-b | 正しく反映 | `finalize` で出す役のタスクを止め、送信側をすべて手放す。理由として挙げた `:941` のループも合っている。 |
| m-c | 正しく反映 | `words: None` にすると `speaker_label` は「自分」「相手」になる（`recorder-domain/src/track.rs:52-61`）。 |
| m-d | 正しく反映 | `uid` → `row_id` の対応を覚え、台本の行を前から順に消費する。再試行や流し切りで同じ `uid` が来ても同じ行になる。 |
| m-e | 反映 | `recent_nodes` を公開して単体テストと共有することを、分割 3 に入れた。置き場所は PR #15 の作者と合わせる。 |
| m-f | 正しく反映 | `StatusDto`（`control-protocol/src/lib.rs:82-96`）に `#[serde(default)]` 付きで足すのは分割 3（コア）、アプリの control_server は分割 4。§5 で rev を読む対象も `last_session_id` のセッションに決めた。 |
| m-g | 正しく反映 | 会議の後の操作でも工場を使う。上書きは `AppSettings` を書き換えず、工場か consumer に渡す所で当てる。 |
| m-h | 正しく反映 | `compile_error!` は `desktop` crate に置き、`desktop` の feature で判定する。 |

反映による新しい矛盾は見つかりませんでした。分割の順序も、次のとおり整合しています。

- 分割 1 と分割 3 は、それぞれコアのタグを上げる。
- 分割 2 は PR #15 のマージ後に始める。
- 分割 4 は、分割 2・3 と `GraphStatus.rev` の後に行う。

## Minor（直さなくても収束の判定は変わりません）

- **r3-m1. §A の書き方が古いまま（L45）**: 「その発話で初めて根拠がそろうノード」と「両端がそろったエッジ」は、r1 の書き方が残っています。§3 の規則（ノードは根拠の最初の `row_id` が今回の `new` にあるもの、エッジは少なくとも一方が今回の `add_node` のもの）に合わせて、「その発話で初めて根拠が出るノードと、それを端点に含むエッジ」と直すことを勧めます。
