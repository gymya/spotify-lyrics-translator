# Spotify Lyrics Translator

macOS 桌面 MVP：讀取 Spotify 桌面版播放資訊，從 LRCLIB 取得同步歌詞，以 Google Cloud Translation Basic v2 或 Microsoft Azure Translator 翻譯成繁體中文，在透明浮動視窗顯示原文與譯文。

## 使用

1. 開啟 Spotify 桌面版與本 App，播放一首歌。
2. macOS 詢問自動化權限時允許讀取 Spotify。若拒絕，可在 App 點「開啟 macOS 自動化權限設定」，允許 Spotify Lyrics Translator 控制 Spotify。開發模式可能顯示 Terminal 或啟動程式的名稱。
3. 若需要翻譯，在 Google Cloud 建立自己的專案、啟用計費與 **Cloud Translation API**，建立限用於此 API 的 key。在本 App 的設定中輸入並儲存。使用 **Basic v2**；不是 Advanced v3，也不使用服務帳戶私鑰。
4. 儲存 key 後會自動翻譯目前歌曲，之後切歌亦會自動翻譯；費用由你的 Google 專案負擔。沒有 key 時仍能顯示原文。
5. 調整透明度、字體大小、原文與譯文字體顏色、時間偏移會自動生效並儲存。正偏移讓歌詞提早，負偏移讓歌詞延後。
6. 點「調整位置」暫時關閉滑鼠穿透，拖曳浮窗後可點「完成調整」。設定視窗失去焦點或關閉時也會自動結束調整，恢復原本的滑鼠穿透設定。視窗位置自動儲存。
7. 關閉設定視窗不會退出。由 macOS 選單列圖示重新開啟設定、切換調整模式、重試或結束 App。

沒有同步歌詞、純音樂、找不到歌詞與 API 錯誤會分別顯示。只有純文字歌詞時不會猜測時間軸。翻譯失敗會保留原文；使用「重新載入」重試，若遇到速率限制，須等畫面提示的時間後再試。

設定頁的歌曲卡片提供「上一首」、「播放／暫停」、「下一首」，也可點擊或拖動進度條跳轉播放位置。需要同樣的 Spotify 自動化權限；播放資訊與歌詞會在下一次輪詢後更新。

## Microsoft Azure Translator F0

1. 在 Azure Portal 建立 **Translator** 資源，定價層選 **F0**。
2. 在 App「翻譯服務」選 **Microsoft Azure Translator（支援 F0）**。
3. 從資源的「金鑰與端點」複製 Key 與位置代碼（例如 `eastasia`）。區域資源必須填 Region；Global 資源留空或填 `global`。
4. 儲存憑證後會翻譯目前歌曲，設定與服務選擇自動儲存。

使用標準 v3 API、自動偵測來源語言、翻譯成 `zh-Hant`。F0 每月免費 200 萬字元，方案與額度由 Azure 管理，App 不會建立或升級 Azure 資源。額度或速率限制會顯示錯誤並保留原文，不自動改用 Google。僅支援公有雲全域端點；不支援自訂／私人端點。

Azure 與 Google 使用各自的 Keychain 項目與翻譯快取，切換服務不會覆寫另一家的 key。兩者沿用保守的每批 128 行／5,000 字元限制。Azure 的純文字回應不做 HTML entity 解碼。

官方參考：[Azure v3 API](https://learn.microsoft.com/en-us/azure/ai-services/translator/text-translation/reference/v3/translate) · [F0 定價與額度](https://azure.microsoft.com/en-us/pricing/details/translator/)

## 日文／英文歌手名稱不同時

精確查詢找不到時，App 會用歌名搜尋 LRCLIB，在設定頁列出長度相差不超過 2 秒的同步歌詞候選的歌名、歌手與專輯（不列出僅文字或純音樂版本）。按「使用此版本」後，SQLite 會記住這首 Spotify 歌曲對應的 LRCLIB ID 與歌詞，下次播放直接使用；翻譯繼續沿用內容雜湊快取。選錯時可按「選擇其他歌詞版本」重新搜尋及更換，不需要清除資料庫。清單開啟後可按「關閉選擇其他版本」，不必選擇任何候選。

搜尋不會自動套用候選或翻譯歌手名稱，也不建立跨歌曲的別名字典。若歌名本身也使用完全不同的語言／羅馬字，僅靠歌名搜尋仍可能找不到。網路錯誤會顯示可重試訊息，不會當成無歌詞快取。

## 開發與建置

需要 macOS 12+、Node.js 22.12+、Rust stable、Xcode Command Line Tools，以及 Spotify 桌面版。

```sh
npm ci
npm run tauri -- dev
```

```sh
npm run build
npm test
npm run tauri -- build --bundles app
```

App 位於 `src-tauri/target/release/bundle/macos/Spotify Lyrics Translator.app`。需要磁碟映像時用 `npm run tauri -- build --bundles dmg`。`npm run dev` 只有瀏覽器介面，無法讀取 Spotify 或 Keychain。

本次開發若沿用暫存 Rust 工具鏈（沒有修改系統 PATH）：

```sh
export PATH="/tmp/lyrics-cargo/bin:$PATH"
export CARGO_HOME=/tmp/lyrics-cargo
export RUSTUP_HOME=/tmp/lyrics-rustup
npm run tauri -- dev
```

暫存目錄可能被系統清除，長期開發請自行安裝 Rust。建好的 App 不需要 Node 或 Rust。

## 資料與隱私

- API key 存在 macOS Keychain 的 `app.spotify-lyrics-translator.desktop` / `google-translation-v2` 或 `azure-translator-v3` 項目。UI 只能得知是否已設定，不會取回 key。移除憑證不會刪除歌詞快取。
- SQLite 及顯示設定位於 `~/Library/Application Support/app.spotify-lyrics-translator.desktop/lyrics.sqlite3`。不寫入任何 API key。
- 歌曲名稱、歌手、專輯、長度傳給 LRCLIB；歌詞文字傳給所選的 Google 或 Microsoft Azure。無遙測、無自建後端、無 Spotify OAuth。
- 翻譯快取鍵包含歌曲 metadata、歌詞內容 SHA-256、服務版本與目標語言（Google v2 / zh-TW 或 Azure v3 / zh-Hant）。同一首歌、同一份歌詞會直接讀快取。
- 查無結果與 API 錯誤不永久快取。優先精確匹配標題／歌手／專輯與兩秒內長度；失敗後用歌名搜尋，不要求歌手或專輯文字一致，讓使用者確認候選。搜尋候選仍須符合兩秒內長度，不自動猜測 live/remix/cover。
- Google 每批最多 128 行、5,000 個字元，空行不送出。網路逾時、翻譯中切歌、部分批次失敗或快取寫入失敗時，已送出的請求仍可能計費，重試也可能產生費用。沒有自動付費重試。
- 沒有內嵌共用憑證，也不擷取 Spotify 歌詞或 Google 翻譯網頁。

## 實作與簡單驗證

- `src-tauri/src/playback.rs`：獨立 PlaybackProvider，macOS AppleScript 每秒單次輪詢，四秒逾時會終止子程序；播放控制使用相同介面，僅允許上一首、下一首、播放、暫停及歌曲範圍內的進度跳轉。
- `lyrics.rs`：LRCLIB 查詢、版本匹配、Retry-After、LRC 多時間標籤／metadata／offset／重複時間處理。
- `sync.rs`：Rust 單調時鐘、二分搜尋；暫停不推進，超過 1.5 秒沒有新觀測便停止推估。
- `translation.rs` / `storage.rs`：Google／Azure 批次翻譯、SQLite 快取、Keychain。歌詞以 Vue 純文字插值呈現。
- `lib.rs`：歌曲切換取消舊載入工作，generation 防止舊資料蓋掉新歌，IPC、視窗與選單列。
- `src/`：Vue + TypeScript + Pinia + Tailwind 設定頁及獨立 overlay。

`npm test` 執行少量無網路 Rust 單元測試：播放資訊單位與無效資料、LRC 解析、版本匹配、seek／暫停／偏移／失去觀測、翻譯批次邊界與快取鍵、SQLite 及設定驗證。測試歌詞為自行撰寫的短句，不呼叫 Google 或產生費用。

快速人工測試：播放 → 暫停 → 拖動進度 → 切歌 → 關閉 Spotify；確認歌詞在下一次觀測後更新。再測滑鼠穿透、拖曳、重啟後設定與快取是否保留。

## 已知限制與發佈

- 目前僅實作 macOS，其他平台播放來源明確回報不支援。沒有 Windows 空殼實作。
- 浮窗沿用 grammar-peek-desktop 的原生 NSPanel 做法：初始化時設定 NonactivatingPanel，將 Tauri 內容視圖移入面板，原視窗只作為隱藏的 WebView／IPC 宿主。顯示、隱藏、拖曳與滑鼠穿透均操作同一個面板。macOS 13+ 使用 CanJoinAllApplications，較舊系統使用 FullScreenAuxiliary。全螢幕／Spaces／多螢幕／Mission Control／睡眠喚醒仍需實機情境驗收；macOS 12 的跨 App 全螢幕支援較有限，也不保證覆蓋系統安全視窗。長歌詞在浮窗維持單行並省略溢出文字；設定頁能看完整目前行。
- 使用 macOS private API 支援透明視窗，目標為直接分發，不是 Mac App Store。
- 未設定開發者簽署或 notarization；本機建置不等於可公開發行的安裝檔。正式分發需提供 Apple 憑證、簽署及公證，重新驗證 Automation 權限。沒有加入 App Sandbox。
- 原生面板位置另存於設定的 panelX／panelY，使用 AppKit 邏輯座標，避免與舊 Tauri 實體座標混用；舊設定首次啟動仍可沿用原位置。升級 Tauri 時需驗證這個原生面板整合。
- `.github/workflows/build.yml` 可手動建置或於版本 tag 建置並上傳 artifacts；不會自動公開 Release。尚未建立遠端 repo 或上傳任何內容。
- 發佈前需選擇開源授權、確認歌詞／譯文及 Spotify 使用條款，並將 LRCLIB User-Agent 補上實際專案首頁或聯絡信箱（目前尚未提供公開專案網址）。

官方參考：[Tauri](https://v2.tauri.app/) · [LRCLIB API](https://lrclib.net/docs) · [Google Basic v2](https://cloud.google.com/translate/docs/reference/rest/v2/translate) · [Google 認證](https://cloud.google.com/translate/docs/authentication)

字幕原文與譯文預設使用半透明黑色背景，可在「浮動歌詞」切換「顯示半透明黑底」，設定會自動儲存；不加描邊或陰影。Azure Region 輸入框在儲存及重開後保持空白，已儲存的區域仍供翻譯使用。原生浮窗透過移動通知保存位置，重啟時保留仍可見的位置。

播放／暫停與進度跳轉會立即顯示操作目標，忽略尚未更新的 Spotify 回報；確認後同步實際狀態，失敗或等待確認逾時則恢復並提示。前端控制測試：`node --test tests/playback-controls.test.mjs`（Node.js 22.18+）。
