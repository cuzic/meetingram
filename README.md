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

## ライセンス

MIT または Apache-2.0 のどちらかを選べます。
