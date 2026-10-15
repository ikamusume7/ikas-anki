## 打包与分发（M4）

本外壳用 **Tauri v2**，产物分两种：

### 1. 可分发目录（本环境能跑的）

`tauri/tools/package.ps1` 打出与 `tauri/src/paths.rs` 的 `for_resource_dir()` 一致的布局：

```
out/package/anki-tauri/
  anki-tauri.exe    主程序
  web/              legacy 页面与静态资源（原 qt/_aqt/data/web）
  sveltekit/        SvelteKit 静态产物（原 out/sveltekit）
  README.txt
```

```powershell
corepack yarn build                  # 先出 out/sveltekit
pwsh -File tauri/tools/package.ps1   # cargo build --release + 组装
```

`tauri/tools/verify-package.ps1` 验证"打出来的目录真的能跑"：用
`ANKI_RESOURCE_DIR` **强制打包模式**（在源码树里跑时默认走仓库模式，光靠默认逻辑测不到
打包布局），断言日志是"以打包模式运行"、`/decks/` 与 `/imgs/...` 都能从包里取到、
页面报到（`domDone`），最后跑一遍 `decks-check.mjs` 做完整交互。

```powershell
pwsh -File tauri/tools/verify-package.ps1            # release
pwsh -File tauri/tools/verify-package.ps1 -Profile debug   # 快速
```

### 2. 安装包（需要联网，本环境没做）

`tauri/tauri.conf.json` 已经把 bundle 配置写好了：

| 配置 | 值 |
| --- | --- |
| `bundle.active` / `targets` | `true` / `["nsis"]` |
| `bundle.icon` | `icons/icon.ico` |
| `bundle.resources` | `../qt/_aqt/data/web/ → web/`、`../out/sveltekit/ → sveltekit/` |
| `bundle.windows.nsis.installMode` | `currentUser` |

有网时：

```powershell
cargo install tauri-cli --version "^2"
cd tauri
cargo tauri build          # 会下载 WiX/NSIS 打包工具，产出 NSIS 安装包
```

**为什么没在这里做**：`tauri build` 需要（a）Tauri CLI、（b）打包工具（NSIS/WiX）的下载，
两者都要联网；本机当前没有。`cargo build --release` 是纯本地编译，所以上面的
"可分发目录"路线可以完整跑通并验证。

### 3. 签名

上游用 Briefcase 流程签 `Anki.exe`；换成 Tauri 后签名对象是
`tauri/target/release/anki-tauri.exe` 与生成的安装包（`signtool` 或 Tauri 的
`bundle.windows.signCommand`）。**未做**，同样受限于证书与网络。

### 4. Android（未做）

`tauri android init/build` 需要 Android SDK/NDK 与 Gradle 依赖下载。需要处理的点已在
`TAURI_HANDOVER.md` §6.4 里列过：`INTERNET` 权限、cleartext（本地 127.0.0.1 服务）、
`getFilesDir()` 替代桌面路径、NDK 交叉编译 rslib。
