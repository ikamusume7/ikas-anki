"use strict";
// 复习界面的键盘操作（对照 `qt/aqt/reviewer.py:595-642` 的 `Reviewer._shortcutKeys()`）。
//
// 宿主注入这份文件（`crate::reviewer::install_reviewer_shortcuts`，用 `include_str!` 引用，
// 保证跑的和被检查的是同一份）。分工与「更多」菜单一致：
//
// - **页面**只把按键翻译成桥接命令（`pycmd("ans")`、`pycmd("ease3")`、`pycmd("rev:mark")`…）；
// - **宿主**执行动作（`src/server/pycmd.rs` 与 `src/reviewer_menu.rs`）。
//
// 因此这里没有第二份状态：是否已显示答案直接看底部栏有没有画出评分按钮
// （`_showEaseButtons` → `showAnswer()` 写进 `#middle`），与 Qt 的 `self.state == "answer"`
// 一一对应。
(function () {
    if (window.__ankiReviewerKeys) {
        return;
    }
    window.__ankiReviewerKeys = true;

    // 宿主在每次会话开始时刷新（`install_reviewer_shortcuts`）：
    // `spacebarRatesCard` 对照 `ProfileManager.spacebar_rates_card()`。
    function config() {
        return window.__ankiReviewerConfig || {};
    }

    function inTextInput(target) {
        if (!target) {
            return false;
        }
        var name = target.tagName;
        return name === "INPUT" || name === "TEXTAREA" || target.isContentEditable === true;
    }

    // 答案面：底部栏的 `#middle` 里已经有评分按钮。
    // 对照 `reviewer-bottom.js` 的 `showAnswer()`（按钮由 `_answerButtons()` 生成）。
    function showingAnswer() {
        var middle = document.getElementById("middle");
        return !!(middle && middle.querySelector("[data-ease]"));
    }

    // 对照 `Reviewer._onAnswerButton()`：取当前焦点按钮的 `data-ease`，
    // 不是 1..4（或没有焦点）就用默认档 3（`_defaultEase()`）。
    function selectedEase() {
        var val = typeof window.selectedAnswerButton === "function"
            ? window.selectedAnswerButton()
            : "";
        return /^[1-4]$/.test(val) ? Number(val) : 3;
    }

    // 对照 `Reviewer.onEnterKey()`（`reviewer.py:658-664`）：
    // 问题面 → 显示答案（类型作答的输入值由桥接脚本附加，见 `src/bridge.rs`）；
    // 答案面 → 按 `spacebar_rates_card` 决定是否用默认档评分。
    function onEnterKey() {
        if (!showingAnswer()) {
            window.pycmd("ans");
            return;
        }
        if (config().spacebarRatesCard === false) {
            return;
        }
        window.pycmd("ease" + selectedEase());
    }

    document.addEventListener("keydown", function (event) {
        // 输入框（类型作答）里的按键全部交给输入框自己：回车由页面的
        // `_typeAnsPress()` 处理（`ts/reviewer/index.ts`），其余是普通字符。
        if (inTextInput(event.target)) {
            return;
        }

        var key = event.key;
        var lower = key.length === 1 ? key.toLowerCase() : key;
        var ctrl = event.ctrlKey || event.metaKey;

        // 组合键（对照 `_shortcutKeys()` 里的 Ctrl+ 项）
        if (ctrl) {
            // Ctrl+Alt+N → 重置卡片；Ctrl+Alt+E → 创建副本（`reviewer.py:626-627`）；
            // Ctrl+Alt+I → 上一张卡片信息（`reviewer.py:634`）
            if (event.altKey) {
                if (lower === "n") {
                    event.preventDefault();
                    window.pycmd("rev:forget");
                } else if (lower === "e") {
                    event.preventDefault();
                    window.pycmd("rev:copy");
                } else if (lower === "i") {
                    event.preventDefault();
                    window.pycmd("rev:prevCardInfo");
                }
                return;
            }
            if (event.shiftKey) {
                // Ctrl+Shift+D → 设置到期日（`reviewer.py:629`）
                if (lower === "d") {
                    event.preventDefault();
                    window.pycmd("rev:due");
                }
                return;
            }
            // Ctrl+Z → 撤销（主窗口 `actionUndo`）；Ctrl+P → 设置（`actionPreferences`）
            if (lower === "z") {
                event.preventDefault();
                window.pycmd("undo");
            } else if (lower === "p") {
                event.preventDefault();
                window.pycmd("preferences");
            } else if (/^[1-7]$/.test(key)) {
                // Ctrl+1..7 → 设旗标（`reviewer.py:617-620` 按 `flags.all()` 生成）
                event.preventDefault();
                window.pycmd("rev:flag:" + key);
            } else if (key === "Delete" || key === "Backspace") {
                // Ctrl+Del（mac 上是 Ctrl+Backspace）→ 删除笔记（`reviewer.py:628`）
                event.preventDefault();
                window.pycmd("rev:deleteNote");
            }
            return;
        }

        if (event.altKey) {
            return;
        }

        switch (key) {
            // 空格 / 回车（含小键盘 Enter）显示答案或评分（`reviewer.py:611-613`）
            case " ":
            case "Enter":
                event.preventDefault();
                onEnterKey();
                return;
            // F5 → 重播音频。音频重播还没实现（`tauri/PARITY.md` 2.2），
            // 但必须拦住默认行为，否则 WebView 会重新加载页面。
            case "F5":
                event.preventDefault();
                return;
        }

        // 1..4 → 评分（对照 `reviewer.py:598-607` 的 `default_answer_keys`，默认就是 1..4）。
        // 答案面之外不评分，与 `_answerCard()` 的 `self.state != "answer"` 守卫一致。
        if (/^[1-4]$/.test(key)) {
            if (showingAnswer()) {
                event.preventDefault();
                window.pycmd("ease" + key);
            }
            return;
        }

        // 字母键按 `event.key` 精确匹配：`Shift+a` 在 Qt 里是「自动前进」
        // （`reviewer.py:640`，本外壳尚未实现），不能顺手当成 `a`。
        switch (key) {
            case "e":
                // 编辑当前卡片（`reviewer.py:610` → `mw.onEditCurrent`）
                event.preventDefault();
                window.pycmd("edit");
                return;
            case "m":
                // 右键菜单（`reviewer.py:614` → `showContextMenu`）
                event.preventDefault();
                window.pycmd("more");
                return;
            case "u":
                // 撤销（`reviewer.py:636`）
                event.preventDefault();
                window.pycmd("undo");
                return;
            // 以下五项是主窗口的全局快捷键（`main.py:1179-1188`）
            case "a":
                // `a` → `mw.onAddCard()`
                event.preventDefault();
                window.pycmd("add");
                return;
            case "b":
                // `b` → `mw.onBrowse()`
                event.preventDefault();
                window.pycmd("browse");
                return;
            case "t":
            case "T":
                // `t` / `Shift+t` → `mw.onStats()`
                event.preventDefault();
                window.pycmd("stats");
                return;
            case "d":
                // `d` → `moveToState("deckBrowser")`
                event.preventDefault();
                window.pycmd("decks");
                return;
            case "s":
                // `s` → `onStudyKey()`（`main.py:1240-1245`）：复习状态下进概览
                event.preventDefault();
                window.pycmd("overview");
                return;
            case "y":
                // `y` → `mw.on_sync_button_clicked()`（`main.py:1187`）
                event.preventDefault();
                window.pycmd("sync");
                return;
            case "*":
                // 标记笔记（`reviewer.py:621`）
                event.preventDefault();
                window.pycmd("rev:mark");
                return;
            case "=":
                // 暂缓笔记（`reviewer.py:622`）
                event.preventDefault();
                window.pycmd("rev:buryNote");
                return;
            case "-":
                // 暂缓卡片（`reviewer.py:623`）
                event.preventDefault();
                window.pycmd("rev:buryCard");
                return;
            case "!":
                // 搁置笔记（`reviewer.py:624`）
                event.preventDefault();
                window.pycmd("rev:suspendNote");
                return;
            case "@":
                // 搁置卡片（`reviewer.py:625`）
                event.preventDefault();
                window.pycmd("rev:suspendCard");
                return;
            case "o":
            case "O":
                // 牌组选项（`reviewer.py:632`）
                event.preventDefault();
                window.pycmd("rev:options");
                return;
            case "i":
            case "I":
                // 卡片信息（`reviewer.py:633`）
                event.preventDefault();
                window.pycmd("rev:cardInfo");
                return;
        }
    });
})();
