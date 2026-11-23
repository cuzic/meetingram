# meetingram

会議を録音しながら文字起こしをして、議論の内容をマインドマップや TOC の思考プロセス図(CRT、対立解消図など)にまとめ、今どんな質問をすればよいかを考える手助けをするツールです。

A desktop tool that records and transcribes meetings while turning the discussion into diagrams (mind maps, TOC Current Reality Trees, Evaporating Clouds) and suggesting what to ask next.

## 今の状態

録音と文字起こしの部分は [1on1-recorder](https://github.com/cuzic/1on1-recorder) のデスクトップアプリ(Dioxus)と CLI を引き継いだものです。図解と質問支援はこれから作ります。

録音・文字起こし・イベント配信・Rhai プラグインの crate は [meetingram-core](https://github.com/cuzic/meetingram-core) にあり、ここからは git 依存(タグ固定)で参照しています。コアを更新したら、`Cargo.toml` の `[workspace.dependencies]` のタグをまとめて上げます。

## 構成

| パス | 内容 |
|------|------|
| `apps/desktop` | デスクトップアプリ(Dioxus) |
| `apps/desktop-cli` | `meetingramctl`。起動中のアプリを録音開始・停止・状態確認する CLI |

## ビルド

```sh
cargo build -p desktop -p desktop-cli
```

- Windows と macOS が対象です。Linux でビルドするには GTK と WebKitGTK の開発用パッケージが必要です。
- meetingram-core の `stt-google` のビルドに `protoc` が必要です。
- 配布用のバンドルは `.github/workflows/desktop-bundle.yml` を手動で実行して作ります。

## データの保存先

設定と録音データは OS のデータディレクトリの下の `meetingram/` に保存します。API キーは OS のキーリングにサービス名 `meetingram` で保存します。1on1-recorder とは別の場所なので、両方を同じ PC で使えます。

## Herdr 経由の要約と議論グラフ

要約と議論グラフで Claude Code（Herdr）または Codex（Herdr）を選べます。議論グラフの初期設定は要約の接続先を引き継ぎます。追加課金の確認方法は接続先ごとに設定できます。初期値の「アプリが自動確認」では、追加課金が無効だと確認できる接続が登録されていなければ送信を止めます。

### 管理者が確認した接続を登録する

1. プロバイダ側で、追加課金へ移行できない専用アカウントと CLI プロファイルを用意します。使用する CLI の外部ツールも無効にします。管理者がこれらの条件を確認した状態を維持してください。
2. そのプロファイルで専用のローカル Herdr コアを起動し、同じ `HC_SESSION` の環境で `hc server status --json` を実行します。確認記録には `core.id` と `pid` の両方を使います。開始済みのコアの環境はあとから変更できないため、認証と接続先が想定どおりの環境で起動してください。
3. アプリのデータディレクトリに `herdr-approvals/claude.json` または `herdr-approvals/codex.json` を作成します。次は記入例です。パス、セッション名、ID、PID、有効期限を確認した接続の値に置き換えます。パスは絶対パス、有効期限は UTC の Unix 秒です。記録にトークンや API キーを含めないでください。

   ```json
   {
     "herdr_binary": "C:/Tools/hc.exe",
     "provider_binary": "C:/Tools/claude.exe",
     "session": "meetingram-claude",
     "core_id": "replace-with-core-id",
     "core_pid": 0,
     "expires_at": 0,
     "additional_billing_disabled": true,
     "external_tools_disabled": true
   }
   ```

4. Meetingram を起動し、対象接続の確認方法に「管理者が確認済みの専用アカウントを使う」を選びます。要約と議論グラフの設定で接続先を選びます。

Meetingram は送信前に確認記録の内容と期限、接続中の Herdr コアの ID と PID、CLI のログイン方式を照合します。追加課金と外部ツールが無効であることは、管理者の確認記録に基づきます。確認記録と CLI プロファイルは管理者が変更できる状態で保護してください。

接続を取り消すには確認記録を削除します。アカウントや CLI の設定を変更する場合は、先に記録を取り消してください。コアを再起動した場合や確認記録を更新した場合は、確認し直して Meetingram を再起動します。

## ライセンス

MIT または Apache-2.0 のどちらかを選べます。
