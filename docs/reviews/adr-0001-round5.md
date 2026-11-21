# ADR 0001 レビュー（round 5、r6 の差分）

対象: cuzic/meetingram docs/adr-0001-r5 @ c463c85（`git diff c6b49ad -- docs/adr/0001-discussion-graph.md`）
（手元の claude 2.1.330 / codex-cli 0.167.0 で、r6 の呼び方を合成の短いプロンプトで試した。会議の本文は使っていない）

## 結論（先に）
converged: **no**。round 4 の M1、M3、m1〜m4 は本文で解消した。M2 は形の上では直ったが、実際には r6 の codex のフラグでも
Web 検索と外部に書き込める道具が残る（N1、Blocker）。M3 の修正で入れた互換の判定は、claude 2.1.330 の `--help` では常に
「互換」になる（N2、Major）。M4 の S2 の条件は、ノード数の 25% 減は測れて意味があるが、60 分の 1 本の 2 つの条件が測れないか自明（N3、Major）。

## 前ラウンドの解消状況
- M1: 解消。§2「プロセスツリーごと止める」（Job Object / `killpg`、止めてから一時ディレクトリを消す）と、タスク 4 の孫のテスト（Linux と Windows の `.cmd`）。細部は N4。
- M2: **未解消（N1 に置き換え）**。3 つの無効化、`-o`、stderr を保持しない、引数の列を固定するテストは入った。しかし、この 3 つでは codex のツールは閉じない。
- M3: 解消。`--setting-sources ""` と `--ignore-user-config` をやめた。coordinator の確認どおり、claude は `--setting-sources` があっても無くても上乗せは同じだった
  （手元で、空の作業ディレクトリ、祖先に `CLAUDE.md` を置いた場合でも 637 トークンで、`CLAUDE.md` は読まれなかった）。
  互換の呼び方の判定は N2。
- M4: 一部解消。B の会議と 60 分の会議が必須になり、条件は「強める前との差」になった。残りは N3。
- m1: 解消（2.8〜3.0k、約 11.5 分、Windows の出典、A の lean は m1 の haiku だけ、B の common 0.3%）。
  「整理で `merged` になったノードは 0〜16」は最終のグラフの `merged` の数で、会議中の `merge` も含む（N5）。
- m2: 解消。m3: 解消（並行の書き込み、`BrokenPipe`、`is_available`、空の引数と空白・日本語のパス、相対名、テスト）。m4: 解消。

## Findings

### N1（Blocker）: r6 の codex のフラグでも、Web 検索と、外部に公開・書き込みできる道具が使える
- 根拠: r6 の codex の行（`-c features.shell_tool=false -c features.view_image=false -c web_search="disabled"`、`-c mcp_servers={}`、`-s read-only`）で、
  「使える道具の名前を全部挙げよ」という system を渡して、手元で実行した（応答は `-o` から読み、`--json` でトークン数を取った）。挙がった道具は次のとおり:
  - `mcp__codex_apps__search_service_web_run`（Web 検索）
  - `mcp__codex_apps__sites_deploy_site_version` と `chatgpt_space_create_page`、`update_page_sharing`（外部に公開・共有）
  - `functions.exec`、`apply_patch`、`collaboration.spawn_agent`、`image_gen__imagegen`、`request_plugin_install` など、全部で 100 以上
  - 消えたのは `exec_command`、`web__run`、`view_image`、`write_stdin` だけ

  `codex_apps` は ChatGPT のアカウントにつないだアプリで、`mcp_servers={}` では消えない。`--ignore-user-config` を付けても同じ一覧になった。
  codex には claude の `--tools ""` に当たる指定が無い。`codex features list` には既定で有効な機能が多い（`apps`、`multi_agent`、`code_mode_host`、`image_generation`、`plugins`、`goals`、`browser_use`、`computer_use`、`hooks`）。
  さらに、未知の `-c features.X=false` はエラーにならず「ignored」と出るだけ（手元で確認）。版が上がって名前が変わると、無効化が黙って外れる。
  §2 の「シェル・画像・Web 検索は `-c` で必ず切る（…外に送らせたりしないため）」は、事実として成り立たない。文字起こしからのプロンプトインジェクションで、会議の内容を外に出せる。
  費用の面でも、この構成の codex は短いプロンプトで入力が 16.7k トークンになる。§「計測」の 9.4k（B の lean）より多い。
- 修正案（ADR とタスク 4 に入れる）:
  1. 無効化を「確かめた一覧」として持ち、少なくとも `features.apps`、`multi_agent`、`code_mode_host`、`image_generation`、`plugins`、`goals`、`browser_use`、`computer_use`、`unified_exec` を足す。
     手元でこれらを足すと、残りは `functions.exec` と `collaboration.*` だけになり、入力は 10.5k に下がった。ただし、まだゼロではない。
  2. 呼び出しを `--json` で受け、ツールの呼び出しのイベント（`item.started` の command / mcp_tool_call / web_search など）が 1 つでも出たら、その時点でツリーごと止めて結果を捨てる。種類は `non_zero_exit` ではなく、たとえば `tool_use` として分ける。
  3. lean で使える codex の版を、検証済みの範囲に固定する。範囲の外では codex の CLI のバックエンドを起動の条件から外すか、owner の確認を取る。
  4. それでも残る道具がある間は、「codex の CLI ではグラフを既定で無効にする」を owner の決定（Q 表）に入れる。

  タスク 4 には、「ツールを呼ぶダミーの codex（`--json` でツールのイベントを出す）で結果が捨てられる」テストを足す。
  また、実 CLI で道具の一覧を確かめる手順を、版を上げるたびの手作業の確認として書く。

### N2（Major）: `--help` による互換の判定は、claude 2.1.330 では常に「互換」になる
- 根拠: §2 は「claude は `--tools` と `--system-prompt-file` が `--help` に無ければ互換」とする。ところが `claude --help`（2.1.330、313 行）には
  `--system-prompt-file` という文字列が無い。`--bare` の説明に `--system-prompt[-file]` とあるだけで、一致を数えると 0 件になる。
  一方、S1 と coordinator の確認では、このフラグは動く。書いてあるとおりに実装すると、最新の claude でも毎回、互換の呼び方（上乗せは約 7 倍）になる。
  タスク 4 のテストはダミーの `--help` を使うので、この食い違いは CI で見つからない。
  codex も、`-o` は `codex exec --help` にしか出ない（`codex --help` には無い）。`exec -` は、説明文の「or if `-` is used」でしか分からない。
- 修正案: 判定は `--help` の文字列ではなく、実際の挙動で行う。たとえば次のどちらか。
  - 起動の条件を調べるときに、空の system ファイルと 1 語の標準入力で lean の呼び方を 1 回試す。
  - lean で呼んで「unknown option」などで終わったら、互換で 1 回やり直して、その結果を覚える。

  `--help` を使うなら、どのコマンドの `--help` から何を探すか（claude は `--tools`、codex は `exec --help` の `--output-last-message`）を書く。
  そのうえで、実 CLI の `--help` の出力を固定したテスト（2.1.330 / 0.167.0 の出力を testdata に置く）で、lean と判定されることを確かめる。

### N3（Major）: タスク 4b の 60 分の 1 本の条件は、測れないか自明。25% 減と再現率の条件は測れるが、ばらつきの扱いが無い
- 根拠:
  - 「整理全体が `120 秒 × 呼び出しの数` に収まる」は、1 回ごとに 120 秒で打ち切る（§7）ので、必ず成り立つ。何も確かめていない。
  - 「issue をまたぐ重複が分割の経路で統合される」は、別々の合成会議をつないだだけでは、まとまりをまたぐ重複がそもそも無い。
    どの組が重複かの正解も無いので、統合されたかを判定できない。
  - 「生きているノードが平均 25% 以上少ない」は測れて、意味もある。A の sonnet は正解の 1.2〜1.5 倍、B は 2〜4 倍なので、正解に近づける向きの目標になっている。
    一方、「再現率の低下が 0.05 以内」は、A の正解 33〜46 ノードでは 1 ノードが 0.02〜0.03 に当たる。1 回ずつの実行の揺れと同じ大きさで、間違った `merge` を 1〜2 件見逃す。
    加えて、強める前と後で増分の抽出を別々に回すと、差には増分のばらつきが混ざる。
- 修正案:
  - 強める前と後の整理は、同じ流し切りの後のグラフ（1 回保存したもの）に対してかける（対の比較）。こうすると増分のばらつきが消える。
    各会議 3 回ずつ回し、会議ごとの値も ADR に載せる。
  - 再現率に加えて「誤った `merge`（`from` と `into` が正解の別のノードに対応する組）の数」を数え、条件にする（例: 6 本で合計 2 件以下）。
  - 60 分の 1 本は、同じ issue が離れた位置にもう一度出るように作る（前半の議題を、後半で言い換えて蒸し返す）。正解に「同じ issue の組」を書き、
    分割の経路でその組が `merge` されることを条件にする。時間の条件は「打ち切りが 0 回」か「整理全体の所要時間を記録する」にする。

### N4（Minor）: Job Object とプロセスグループの細部
- 根拠と修正案:
  - Windows では、起動してから Job Object に入れるまでの間に、cmd.exe が node を起動しうる。この間に起動した孫は Job に入らない。
    `CREATE_SUSPENDED` で起動し、Job に入れてから再開する、と書く。`std` / `tokio` はスレッドのハンドルを返さないので、再開の方法（`NtResumeProcess`、または `windows` crate で `CreateProcessW` を直接呼ぶ）も決めておく。
  - ツリーごと止めるのは、時間の上限と作り直しの時だけでなく、呼び出しの future が drop されたとき（タスクの中断、アプリの終了）もそうする。
    今の `kill_on_drop` の代わりになる guard を置く。Unix では、アプリが落ちるとプロセスグループが残るので、次の起動時の一時ディレクトリの掃除とあわせて扱いを書く。

### N5（Minor）: 利用者の hooks・plugins が本文を受け取ることと、数値の言い回し
- 根拠:
  - 利用者の設定を残したので、claude の `UserPromptSubmit` などの hooks と、有効な plugins の hooks が、45 秒ごとに文字起こしを含む user を受け取る。
    codex も `hooks` が既定で有効。今の要約と同じ挙動で利用者自身の設定ではあるが、§「影響」のプライバシーに書いていない。
  - §3 の「整理で `merged` になったノードは 1 会議 0〜16 件」は、最終のグラフの `merged` の数で、会議中の `merge` を含む。
  - §「計測」の codex の 9.4k は、今の codex のアカウントの状態では再現しない（N1 の 16.7k）。
- 修正案: プライバシーに「CLI の利用者の hooks・plugins も本文を受け取る」と書く。「整理で」を「最終のグラフで（会議中を含む）」に直す。
  codex の費用は N1 の構成で S2 のときに測り直すと書く。

## 判定
converged: no（Blocker 1 / Major 2 / Minor 2 / nit 0）
