(function () {
    if (window.__ankiReviewerMenu) {
        return;
    }
    window.__ankiReviewerMenu = true;
    // 菜单要开在鼠标位置（对照 Qt 的 `m.popup(QCursor.pos())`），
    // 而"更多"按钮与右键的坐标只有页面知道，所以这里记下来给宿主用。
    window.__ankiPointer = { x: 40, y: 40 };
    document.addEventListener(
        "mousedown",
        function (event) {
            window.__ankiPointer = { x: event.clientX, y: event.clientY };
        },
        true,
    );

    function inTextInput(target) {
        if (!target) {
            return false;
        }
        var name = target.tagName;
        return name === "INPUT" || name === "TEXTAREA" || target.isContentEditable === true;
    }

    // 菜单里的一项：label + 快捷键提示 + 勾选态 + 点击后要发的桥接命令。
    // 带子菜单的项用 `<div>` 包一层 —— `<button>` 里不能再放 `<button>`，
    // 浏览器会自己修 DOM，悬停展开就不稳了。
    function item(entry, onPick) {
        var row = document.createElement(entry.submenu ? "div" : "button");
        row.className = "rev-menu-item";
        row.dataset.action = entry.action || entry.label;
        if (entry.submenu) {
            row.classList.add("rev-menu-parent");
        }
        var check = document.createElement("span");
        check.className = "rev-menu-check";
        check.textContent = entry.checked ? "✓" : "";
        var label = document.createElement("span");
        label.className = "rev-menu-label";
        label.textContent = entry.label;
        row.appendChild(check);
        row.appendChild(label);
        if (entry.shortcut) {
            var shortcut = document.createElement("span");
            shortcut.className = "rev-menu-shortcut";
            shortcut.textContent = entry.shortcut;
            row.appendChild(shortcut);
        }
        if (entry.submenu) {
            var submenu = buildMenu(entry.submenu, onPick, true);
            row.appendChild(submenu);
            row.addEventListener("mouseenter", function () {
                submenu.style.display = "block";
            });
            row.addEventListener("mouseleave", function () {
                submenu.style.display = "none";
            });
        } else {
            row.addEventListener("click", function () {
                closeMenu();
                onPick(entry.action);
            });
        }
        return row;
    }

    function buildMenu(items, onPick, isSubmenu) {
        var menu = document.createElement("div");
        menu.className = isSubmenu ? "rev-submenu" : "rev-menu";
        items.forEach(function (entry) {
            if (!entry) {
                var separator = document.createElement("hr");
                separator.className = "rev-menu-separator";
                menu.appendChild(separator);
                return;
            }
            menu.appendChild(item(entry, onPick));
        });
        return menu;
    }

    function closeMenu() {
        var existing = document.querySelector(".rev-menu");
        if (existing) {
            existing.remove();
        }
    }

    window.showReviewerMenu = function (spec) {
        closeMenu();
        var menu = buildMenu(spec.items, function (action) {
            window.pycmd("rev:" + action);
        });
        menu.style.left = Math.max(0, Math.min(window.__ankiPointer.x, window.innerWidth - 260)) + "px";
        menu.style.top = Math.max(0, Math.min(window.__ankiPointer.y, window.innerHeight - 320)) + "px";
        document.body.appendChild(menu);
        // 点别处关闭（对照 QMenu 的行为）
        setTimeout(function () {
            document.addEventListener(
                "mousedown",
                function handler(event) {
                    if (!menu.contains(event.target)) {
                        document.removeEventListener("mousedown", handler, true);
                        closeMenu();
                    }
                },
                true,
            );
        }, 0);
    };

    // 表单对话框（「重置卡片…」「设置到期日…」用）：Qt 那边是 QDialog，
    // 这里画一个最小表单，确认后把值拼成 `rev:<动作>:<参数>`。
    window.showReviewerDialog = function (spec) {
        var overlay = document.createElement("div");
        overlay.className = "rev-dialog-overlay";
        var dialog = document.createElement("div");
        dialog.className = "rev-dialog";
        var title = document.createElement("h5");
        title.textContent = spec.title;
        dialog.appendChild(title);
        var fields = [];
        spec.fields.forEach(function (field) {
            var row = document.createElement("div");
            row.className = "rev-dialog-row";
            if (field.kind === "check") {
                var box = document.createElement("input");
                box.type = "checkbox";
                box.checked = !!field.value;
                var label = document.createElement("label");
                label.textContent = field.label;
                row.appendChild(box);
                row.appendChild(label);
                fields.push({ kind: "check", input: box });
            } else {
                var textLabel = document.createElement("label");
                textLabel.textContent = field.label;
                var input = document.createElement("input");
                input.type = "text";
                input.value = field.value || "";
                row.appendChild(textLabel);
                row.appendChild(input);
                fields.push({ kind: "text", input: input });
            }
            dialog.appendChild(row);
        });
        var footer = document.createElement("div");
        footer.className = "rev-dialog-footer";
        var cancel = document.createElement("button");
        cancel.textContent = spec.labels.cancel;
        cancel.addEventListener("click", function () {
            overlay.remove();
        });
        var confirm = document.createElement("button");
        confirm.className = "rev-primary";
        confirm.textContent = spec.labels.confirm;
        confirm.addEventListener("click", function () {
            var values = fields.map(function (field) {
                return field.kind === "check" ? (field.input.checked ? "1" : "0") : field.input.value;
            });
            overlay.remove();
            window.pycmd("rev:" + spec.action + ":" + values.map(encodeURIComponent).join(","));
        });
        footer.appendChild(cancel);
        footer.appendChild(confirm);
        dialog.appendChild(footer);
        overlay.appendChild(dialog);
        document.body.appendChild(overlay);
        var firstInput = dialog.querySelector("input[type=text]");
        if (firstInput) {
            firstInput.focus();
        }
    };

    // 卡片上右键 = 打开这个菜单（对照 Qt 给 review webview 设的 context menu policy）
    document.addEventListener("contextmenu", function (event) {
        if (inTextInput(event.target)) {
            return;
        }
        window.__ankiPointer = { x: event.clientX, y: event.clientY };
        event.preventDefault();
        window.pycmd("more");
    });

    var style = document.createElement("style");
    style.textContent = [
        ".rev-menu, .rev-submenu { position: fixed; z-index: 3000; min-width: 13rem;",
        "  padding: 0.25rem; background: var(--canvas, #fff); border: 1px solid var(--border, #ccc);",
        "  border-radius: 6px; box-shadow: 0 4px 16px rgba(0,0,0,.25); text-align: start; }",
        ".rev-submenu { display: none; position: absolute; left: 100%; top: -0.25rem; }",
        // 卡片页会把内容居中，菜单要显式左对齐（`<button>` 默认也是居中的）
        ".rev-menu-item { display: flex; gap: 0.5rem; align-items: center; width: 100%;",
        "  padding: 0.2rem 0.4rem; color: var(--fg, #000); background: transparent; border: none;",
        "  border-radius: 4px; cursor: pointer; font-size: 0.95rem; text-align: start;",
        "  justify-content: flex-start; }",
        ".rev-menu-item:hover { background: var(--selected-bg, #e6e6e6); }",
        ".rev-menu-item.rev-menu-parent { position: relative; }",
        ".rev-menu-check { width: 1rem; text-align: center; }",
        ".rev-menu-label { flex: 1; white-space: nowrap; }",
        ".rev-menu-shortcut { color: var(--fg-subtle, #888); font-size: 0.85em; }",
        ".rev-menu-separator { margin: 0.25rem 0; border: none; border-top: 1px solid var(--border, #ddd); }",
        ".rev-dialog-overlay { position: fixed; inset: 0; z-index: 3100; display: flex;",
        "  align-items: center; justify-content: center; background: rgba(0,0,0,.35); }",
        ".rev-dialog { min-width: 20rem; padding: 1rem; background: var(--canvas, #fff);",
        "  border-radius: 8px; box-shadow: 0 8px 24px rgba(0,0,0,.3); text-align: start; }",
        ".rev-dialog h5 { margin: 0 0 0.75rem; }",
        ".rev-dialog-row { display: flex; gap: 0.5rem; align-items: center; margin-bottom: 0.5rem; }",
        ".rev-dialog-row input[type=text] { flex: 1; padding: 0.25rem 0.4rem; }",
        ".rev-dialog-footer { display: flex; justify-content: flex-end; gap: 0.5rem; margin-top: 0.75rem; }",
        ".rev-dialog-footer button { padding: 0.25rem 0.75rem; }",
        ".rev-primary { background: var(--selected-bg, #2f6fb5); color: var(--selected-fg, #fff); border: none; }",
    ].join("\n");
    document.head.appendChild(style);
})();