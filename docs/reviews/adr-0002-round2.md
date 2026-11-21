# ADR 0002 r2 のレビュー（round 2）

対象: `docs/adr/0002-e2e-scripted-backends.md` r2（aabf100）。照合先は round 1 と同じで、meetingram-core main（440f215）、app の main、PR #15 の head です。

**結論**: round 1 の Blocker 1 件と Major 5 件は、すべて解消されています。ただし r2 で書き足したエッジの規則に新しい穴が 1 件（Major）あるため、まだ収束していません。この 1 件を直せば収束と判断してよい見込みです。

## round 1 の指摘の解消状況

| 指摘 | 状況 | 確認したこと |
| --- | --- | --- |
| B1 ヒント | 解消 | §「決めない」に移し、§5 からも外した |
| M1 STT の口 | 解消。形は m-a で補足 | 口を `run_windows_capture_session` に置き、両方の分岐の前で分ける。プロバイダを共有している 4 か所はコード（`live_transcription.rs:887-902`、`:1052`、`:1056`）と一致する |
| M2 LLM の工場 | 解消 | 合意どおり工場が `Backend` を直接返し、Q6 と `is_available` を通らない。差し込む先 `start_recording_with`（PR #15 `graph_consumer.rs:305-350`）は、`enabled()` を工場の外で先に判定している（`:320-323`）。ADR §3 の記述と一致する |
| M3 エッジの根拠と確信度 | 一部解消 | 新しく入った規則に M-1 の穴がある |
| M4 チャネルと再接続 | 解消。細部は m-b | |
| M5 単体テストと CI | 解消 | テストに使う `Graph` は `Default` があり、フィールドはすべて pub（`model.rs:123-131`）。`build_incremental` と `apply_patch` も公開されている（`lib.rs:10-16`）。Linux で組める |
| m1〜m12 | 反映済み | 残りは m-c〜m-h |

正解の 3 本で、M-1 の修正後の規則（下記）を使ってシミュレートしました。区切りの件数を 1〜12 件のどれにしても、ノードもエッジも全部出ました（エッジ 32/32、36/36、35/35）。文脈の窓は「未解決の issue ＋直近 10 rev」とし、PR #15 `recent_nodes` を近似しています。§A の「最終形は毎回正解グラフと同じ」は、M-1 を直せば成り立ちます。

---

## Major

### M-1. 両端が `context` にあるエッジを毎回返してしまい、却下が出る（§3 L124-126）

規則は「両端が `tmp` か `context` のノードでそろったときに返す」です。この書き方だと、両端とも `context` にあるエッジも条件を満たします。ところが `context` の行に載るのは `nid`・`kind`・`status`・`text`・`parent_nid` だけで（`prompt.rs:86-91`）、すでにあるエッジは載りません。偽の LLM は状態を持たないので、そのエッジを出したかどうかを知る手段がありません。

その結果、一度張ったエッジを、両端が窓にある間は毎回返します。そのときの `grounds`（後からそろった端点の最初の根拠）は、もう `new` にも `reference` にもありません。このため `GroundOutsideInput`（`patch.rs:269-273`）で落ちます。根拠が通ったとしても、次は `EdgeDuplicate`（`patch.rs:339-346`）で落ちます。§5 の単体テストの「却下 0 件」と食い違い、本番の経路でも毎回却下が数えられます。

**修正案**: 規則を次のように書き換えます。

> 端点の少なくとも一方が今回の `add_node`（`tmp`）で、もう一方が `tmp` か `context` のノードのときだけ返す。両端とも `context` のエッジは返さない。

この規則なら、`grounds` は必ず今回の `new` にある発話になります。上のシミュレーションも、この規則で行いました。

---

## Minor

- **m-a. `LiveTranscriptionOptions` を置く feature（§2 L85-87）**: 「`live-transcription` feature の中だけ」と書いてあります。しかし `run_windows_capture_session` は、`windows-supervisor` だけの構成でもコンパイルされます（`windows_session.rs:69-85`）。そこで呼ぶ stub も `silence_gate_enabled` を受け取ります（`live_transcription.rs:2827-2840`）。したがって構造体そのものは常に定義し、`stt_override` のフィールドと、それを設定するメソッド（例: `with_stt_override`）だけを `#[cfg(feature = "live-transcription")]` にしてください。作るのは `Default` とビルダーにします。cfg 付きのフィールドがあると、構造体リテラルで書けない構成が出るためです。
- **m-b. `finalize` で閉じること（§2 L101-102）**: `run_live_transcription` のループは、両トラックの events が `None` になるまで終わりません（`:941`）。`finalize` の後には時間の上限もありません。`finalize_with_timeout` が抑えるのは `finalize` の呼び出しそのものだけです（`:1005-1010`）。出す役のタスクが送信側の複製を持ったまま残ると、録音の停止が終わらなくなります。「`finalize` で出す役のタスクを止め、送信側をすべて手放してから戻る」と明記してください。
- **m-c. 話者の情報（§2）**: `FinalTranscript` の `words` は `None`（または `speaker: None`）にすると書いてください。`speaker` があるとラベルが「自分 (話者1)」になり（`recorder-domain/src/track.rs:52-61`）、§3 の（`speaker_label`、本文）による照合が全部外れます。
- **m-d. 「その組の出現順」（§3 L120）**: プロンプトには `new` と `reference` しか載らないので、過去の出現回数はプロンプトから数えられません。偽の LLM がセッションごとに「`uid` → `row_id`」の対応を覚え、台本の行を前から順に消費する、と書いてください（再試行や流し切りで同じ `uid` がもう一度来ても、同じ行を返す）。なお、重なる発話（design で 6 行、budget で 4 行、one-on-one で 9 行）は、どれも正解の根拠ではないので、結果は変わりません。
- **m-e. 単体テストの窓**: 「直近 10 rev」を決める `recent_nodes` は、PR #15 の中で private です。テストが独自に書き直すと、本番とずれます。解析の関数と一緒にコアから公開するか、アプリの中で共有してください。
- **m-f. ctl に rev を載せる作業はコアの変更（§5 L152-153、分割 4）**: `StatusDto` は `control-protocol`（コア）にあります（`lib.rs:82-96`）。分割 4 は「アプリ」とされていますが、実際にはコアの PR とタグがもう 1 つ要ります。新しいフィールドには `#[serde(default)]` を付けます。§5 の「停止後の rev」は `last_session_id` のセッションの値だと書いてください。
- **m-g. 工場を使う経路**: グラフのバックエンドは、録音の開始（`start_recording`）のほかに、会議の後の操作（`post_meeting_with`。続きを抽出、再文字起こしの後、作り直し）でも組み立てます。`graph()` をこれらすべての経路で使う、と書いてください。上書きを「メモリ上だけ」にする件も、`AppSettings` の Mutex を書き換えると設定画面で保存したときに一緒に保存されてしまいます。上書きは工場か `Window` を作る所で当ててください。
- **m-h. `compile_error!` を置く crate（§1）**: `desktop` crate の feature で判定する、と書いてください。`apps/e2e-scripted` に置くと、ワークスペース全体の release ビルドでこの crate が単体でコンパイルされるときの扱いが曖昧になります。

---

**判定**: Major 1 件（M-1）が残っているため、未収束です。M-1 の規則を直したうえで、m-a と m-b も直しておくことを勧めます。
