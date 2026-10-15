// Copyright: Ankitects Pty Ltd and contributors
// License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html

/**
 * 导出对话框背后的公共逻辑：格式映射、`ExportLimit`、问宿主要保存路径、调导出 RPC。
 *
 * 两个入口共用（对话框本身是 `ExportDialog.svelte`）：
 * - 浏览器「更多」→ 导出：范围是「选中的笔记 / 全部集合」（`browser.py:377`）；
 * - 牌组列表的齿轮 → 导出：范围是「该牌组 / 全部集合」（`deckbrowser.py:309-327` →
 *   `main.py:1398-1401` 的 `onExport(did=...)`）。
 *
 * 对照 `qt/aqt/import_export/exporting.py` 的 `ExportDialog` / 四个 `Exporter`。
 */
import { openFilePickerRequest } from "@generated/anki/frontend_pb";
import { Empty, String as GenericString } from "@generated/anki/generic_pb";
import { ExportLimit } from "@generated/anki/import_export_pb";
import { NoteIds } from "@generated/anki/notes_pb";
import {
    exportAnkiPackage,
    exportCardCsv,
    exportCollectionPackage,
    exportNoteCsv,
} from "@generated/backend";
import * as tr from "@generated/ftl";
import { postProto } from "@generated/post";

import type { ExportChoice } from "./menu";

/** 导出的上下文：浏览器给选中的笔记，牌组列表给它那一行牌组。 */
export interface ExportContext {
    /** 浏览器里选中的笔记 id（`selection` 范围用） */
    noteIds?: number[];
    /** 从牌组列表导出的那个牌组（`deck` 范围 + 建议文件名用） */
    deck?: { id: bigint; name: string };
}

/** Qt 的时间戳格式（`exporting.py:179` 的 `%Y-%m-%d@%H-%M-%S`）。 */
export function timestampSuffix(now = new Date()): string {
    const pad = (value: number) => `${value}`.padStart(2, "0");
    return [
        `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`,
        `${pad(now.getHours())}-${pad(now.getMinutes())}-${pad(now.getSeconds())}`,
    ].join("@");
}

/**
 * 建议的文件名（对照 `exporting.py:173-180` 的 `filename()`）：从牌组导出时用牌组名
 * （把 Windows 非法字符换成 `_`，上游的正则 `[\\\\/?<>:*|"^]`），否则用
 * `集合-<时间戳>`。
 */
export function suggestedFilename(extension: string, deckName?: string): string {
    if (deckName) {
        return `${deckName.replace(/[\\/?<>:*|"^]/g, "_")}.${extension}`;
    }
    return `${tr.exportingCollection()}-${timestampSuffix()}.${extension}`;
}

/**
 * 格式 → 扩展名 / 保存框上的筛选名（对照各 `Exporter` 的 `extension` + `name()`，
 * `exporting.py:232-365`）。
 *
 * 必须是函数：`tr.*` 要等 `setupI18n` 之后才有内容（§5.22 / §5.26）。
 */
export function exportFormatOf(format: ExportChoice["format"]): {
    extension: string;
    label: string;
} {
    switch (format) {
        case "apkg":
            return { extension: "apkg", label: tr.exportingAnkiDeckPackage() };
        case "colpkg":
            return { extension: "colpkg", label: tr.exportingAnkiCollectionPackage() };
        case "notes-txt":
            return { extension: "txt", label: tr.exportingNotesInPlainText() };
        case "cards-txt":
            return { extension: "txt", label: tr.exportingCardsInPlainText() };
    }
}

/**
 * 范围 → `ExportLimit`。
 *
 * 注意 `ExportLimit` 里还有一个同名的 `limit` oneof，构造时要嵌一层 —— 少嵌一层不会
 * 报错，而是**静默导出整个集合**（见 TAURI_HANDOVER §5.33）。这里用生成的类显式构造，
 * 让类型检查帮忙挡住这种写法。
 */
export function limitFor(choice: ExportChoice, context: ExportContext): ExportLimit {
    if (choice.scope === "deck" && context.deck) {
        return new ExportLimit({
            limit: { case: "deckId", value: context.deck.id },
        });
    }
    if (choice.scope === "selection" && context.noteIds && context.noteIds.length > 0) {
        return new ExportLimit({
            limit: {
                case: "noteIds",
                value: new NoteIds({ noteIds: context.noteIds.map(BigInt) }),
            },
        });
    }
    return new ExportLimit({ limit: { case: "wholeCollection", value: new Empty({}) } });
}

/**
 * 问宿主要一个保存路径。
 *
 * `saveFileDialog` 是**外壳新增的平台端点**（上游没有：Qt 的导出对话框直接用原生
 * `getSaveFile()`，`exporting.py:119-140`）。请求复用 `OpenFilePickerRequest`，
 * 其中 `key` 按外壳约定传建议的文件名（见 `platform/dialogs.rs` 的注释）。
 * 用户取消时返回空串。
 */
export async function askSavePath(
    filename: string,
    extension: string,
    filterLabel: string,
): Promise<string> {
    const request = new openFilePickerRequest({
        title: tr.actionsExport(),
        key: filename,
        filterDescription: filterLabel,
        extensions: [extension],
    });
    const response = await postProto("saveFileDialog", request, GenericString);
    return response.val;
}

/**
 * 按对话框的选择真的导出，返回要显示的状态文案（用户取消时是空串）。
 *
 * 对照 `exporting.py:142-160` 的 `options()` 与四个 `Exporter.export()`。
 */
export async function performExport(
    choice: ExportChoice,
    context: ExportContext,
): Promise<string> {
    const { extension, label } = exportFormatOf(choice.format);
    const path = await askSavePath(
        suggestedFilename(extension, context.deck?.name),
        extension,
        label,
    );
    if (!path) {
        return ""; // 用户取消
    }
    switch (choice.format) {
        case "colpkg":
            // colpkg 没有 `limit`（整个集合），见 `ColpkgExporter.export()`
            await exportCollectionPackage({
                outPath: path,
                includeMedia: choice.includeMedia,
                legacy: choice.legacy,
            });
            // 对照 `exporting.py:248`：`exporting_collection_exported()`
            return tr.exportingCollectionExported();
        case "apkg": {
            const exported = await exportAnkiPackage({
                outPath: path,
                options: {
                    withScheduling: choice.includeScheduling,
                    withDeckConfigs: choice.includeDeckConfigs,
                    withMedia: choice.includeMedia,
                    legacy: choice.legacy,
                },
                limit: limitFor(choice, context),
            });
            // 对照 `exporting.py:286`：`exporting_note_exported(count=…)`
            return tr.exportingNoteExported({ count: exported.val });
        }
        case "notes-txt": {
            // `NoteCsvExporter.export()`（`exporting.py:317-337`）
            const exported = await exportNoteCsv({
                outPath: path,
                withHtml: choice.includeHtml,
                withTags: choice.includeTags,
                withDeck: choice.includeDeck,
                withNotetype: choice.includeNotetype,
                withGuid: choice.includeGuid,
                limit: limitFor(choice, context),
            });
            return tr.exportingNoteExported({ count: exported.val });
        }
        case "cards-txt": {
            // `CardCsvExporter.export()`（`exporting.py:349-365`）
            const exported = await exportCardCsv({
                outPath: path,
                withHtml: choice.includeHtml,
                limit: limitFor(choice, context),
            });
            // 对照 `exporting.py:355`：`exporting_card_exported(count=…)`
            return tr.exportingCardExported({ count: exported.val });
        }
    }
}
