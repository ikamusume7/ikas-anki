// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

/**
 * 右键菜单与通用表单弹框的类型。
 *
 * 单独放在 `.ts` 里是因为 Svelte 组件不方便对外导出类型（`ContextMenu.svelte` 与
 * `+page.svelte` 都要用同一份定义）。
 */

/** 菜单里的一项。对照 Qt 的 `QAction`：文案、快捷键提示、勾选态、禁用态。 */
export interface MenuItem {
    label: string;
    /**
     * 动作名，渲染成 `data-action`。
     *
     * Qt 那边靠 `browser.ui` 里的 action 名（`actionToggle_Mark` 等）定位动作，
     * 网页这边没有 action 对象，所以给一个显式的标识 —— 验证脚本
     * （`tauri/tools/browser-check.mjs`）按它点菜单项，不用去匹配本地化文案。
     */
    action?: string;
    /** 只作为提示显示（快捷键由页面的 `onKeydown` 真正实现） */
    shortcut?: string;
    disabled?: boolean;
    /** 勾选态（标记 / 暂停 / 埋藏 / 旗标） */
    checked?: boolean;
    /** 点击后执行（菜单会先关掉自己） */
    run: () => void;
}

/** 菜单分区（Qt 菜单里的分隔线）。 */
export interface MenuSection {
    items: MenuItem[];
}

/** 通用表单弹框里的一项。 */
export type FormField =
    | { kind: "text"; label: string; value: string }
    | { kind: "number"; label: string; value: number; min?: number }
    | { kind: "check"; label: string; value: boolean };

/** 表单弹框点击「确定」时交回的值（按字段顺序）。 */
export type FormValues = { text: string; number: number; check: boolean }[];

/** 导出对话框的选择（对照 `qt/aqt/import_export/exporting.py` 的 `ExportOptions`）。 */
export interface ExportChoice {
    /**
     * `apkg` = Anki 牌组包、`colpkg` = 整个集合、
     * `notes-txt` = 笔记纯文本（`NoteCsvExporter`，制表符分隔）、
     * `cards-txt` = 卡片纯文本（`CardCsvExporter`）。
     */
    format: "apkg" | "colpkg" | "notes-txt" | "cards-txt";
    /**
     * 导出范围：`selection` = 浏览器里选中的笔记、`deck` = 从牌组列表齿轮导出的那个
     * 牌组、`all` = 整个集合。colpkg 只有「整个集合」。
     */
    scope: "selection" | "all" | "deck";
    includeScheduling: boolean;
    includeDeckConfigs: boolean;
    includeMedia: boolean;
    /** 「HTML 与媒体引用」（`exporting-include-html-and-media-references`） */
    includeHtml: boolean;
    includeTags: boolean;
    includeDeck: boolean;
    includeNotetype: boolean;
    includeGuid: boolean;
    /** 「支持旧版 Anki」（`legacy`） */
    legacy: boolean;
}
