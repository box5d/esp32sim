# CoreS3スマホカメラの分割受信修正（2026-10-20）

## 修正した不具合

カメラの1枚は160×120のRGBAと5バイトのヘッダーで76,805バイトになる。エミュレーターのWebSocket受信処理は、最初のbinaryフレームを即座に渡し、continuationフレームを無視していた。画像が分割されると画像長が足りず、ボードの画像が更新されないため初期カラーバーが残る。

`~/m5cores3-emu/esp32sim/esp-soc/src/web.rs`でFINまで画像を結合し、完成したメッセージだけを公開するよう変更した。不完全な次の画像で直前の正常な画像を失わない。同じ結合処理でtextにも対応し、メッセージ全体にも既存の8MiB上限を適用した。

## 再現と検証

- 実TCP受信の回帰試験は、変更前に65,536バイトしか受け取れず、期待値76,805バイトで失敗した。[変更前ログ](../../../agy-web-console/docs/maintenance-evidence/cores3-camera/fragmentation-before.txt)。
- 回帰試験は3断片の結合、断片間の制御フレーム、textの結合、古い完成画像の置換、不完全画像の破棄を確認する。
- `cargo test --locked --offline -p esp-soc -p esp32s3`：125テスト成功。[ログ](../../../agy-web-console/docs/maintenance-evidence/cores3-camera/regression-tests.txt)。
- 実ChromiumのgetUserMediaに赤／緑の既知の動画を入力し、製品のCoreS3 Studioから実WebSocketへ送信した。公式デモv0.5のROM起動後、タッチでカメラ画面を開き、Rust側の受信プレビューだけでなく、ファームウェアのカメラ取得→DMA→LCDまで色が変わることを確認した。
- 通常送信は変更前・変更後とも成功した。分割がすべてのブラウザーで発生するとは断定しない。
- 分割を行う隔離プロキシを通した試験では、変更前は送信中でも受信映像が更新されず失敗し、変更後は赤・緑とも受信プレビューとLCDへ反映された。[変更前](../../../agy-web-console/docs/maintenance-evidence/cores3-camera/baseline-fragmented/result.json)、[変更後](../../../agy-web-console/docs/maintenance-evidence/cores3-camera/fixed-fragmented/result.json)、[LCDの赤](../../../agy-web-console/docs/maintenance-evidence/cores3-camera/fixed-fragmented/guest-red.png)、[LCDの緑](../../../agy-web-console/docs/maintenance-evidence/cores3-camera/fixed-fragmented/guest-green.png)。
- 試験は別ポートの自分のエミュレーター・ブラウザーだけで実施し、終了後に停止した。カメラ映像は人工の動画で、スマホの実カメラは使用していない。

## 配置・未確認

ソースと修正版releaseバイナリを現行のエミュレーターへ配置した。[ハッシュ](../../../agy-web-console/docs/maintenance-evidence/cores3-camera/deployment.json)。既設バイナリは隔離作業フォルダの`camera-evidence/installed-before`へバックアップした。再実行用のブラウザー試験はエミュレーターの`tools/cores3-camera-browser.cjs`に保存した。

現在の8088番のエミュレーターは旧バイナリで稼働していることを実行ファイルのハッシュで確認した。共有作業ルールで、自分が起動したと確認できないプロセスの停止・再起動には明示的な依頼が必要なため、まだ再起動していない。Web Consoleサーバーの変更・再起動は不要。

ユーザーのスマホで実際に画像が分割されているか、実カメラの許可・入力プレビューの状態は未確認。端末の症状を完全解消したとはまだ判定しない。
