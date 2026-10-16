<!--
Copyright: Ankitects Pty Ltd and contributors
License: GNU AGPL, version 3 or later; http://www.gnu.org/licenses/agpl.html
-->
<script lang="ts">
    import "../deck-options/deck-options-base.scss";
    import { ExperimentalFeatureFlag, Preferences, Preferences_Scheduling } from "@generated/anki/config_pb";
    import { Empty } from "@generated/anki/generic_pb";
    import { HelpPageLinkRequest_HelpPage } from "@generated/anki/links_pb";
    import { getPreferences, helpPageLink, openLink, setPreferences } from "@generated/backend";
    import { translate as translateRaw } from "@generated/ftl-helpers";
    import { bridgeCommand } from "@tslib/bridgecommand";
    import { ModuleName, setupI18n } from "@tslib/i18n";
    import {
        getMeta,
        getProfileConfig,
        setMeta,
        setProfileConfig,
    } from "@tslib/profile";
    import { onMount } from "svelte";

    import Switch from "$lib/components/Switch.svelte";
    import type { PreferenceStore } from "$lib/sveltelib/preferences";

    import LabItem from "./LabItem.svelte";

    export let data: { labPerfs: PreferenceStore<any> };
    const labPerfs = data.labPerfs;

    // 系统字号：`standard_css()` 里有 `:root.system-font-size` 这套规则
    // （`qt/aqt/webview.py:688`），SvelteKit 页要自己加上这个类
    document.documentElement.classList.add("system-font-size");

    // 分页顺序照 `forms/preferences.ui` 的 QTabWidget
    type Tab = "appearance" | "review" | "editing" | "network" | "backups" | "thirdparty" | "labs";
    const tabs: { id: Tab; key: string }[] = [
        { id: "appearance", key: "preferences-appearance" },
        { id: "review", key: "preferences-review" },
        { id: "editing", key: "preferences-editing" },
        { id: "network", key: "preferences-tab-synchronisation" },
        { id: "backups", key: "preferences-backups" },
        { id: "thirdparty", key: "preferences-third-party-services" },
        { id: "labs", key: "preferences-experimental-features-tab" },
    ];
    let tab: Tab = "appearance";

    // meta（`prefs21.db` 的 `_global` 行）
    let theme = 0;
    let lang = "";
    let uiScale = 100;
    let reduceMotion = true;
    let minimalistMode = false;
    let hideTopBar = false;
    let topBarHideMode = 0;
    let hideBottomBar = false;
    let bottomBarHideMode = 0;
    let spacebarRatesCard = true;
    let checkForUpdates = true;
    let checkForAddonUpdates = true;

    // profile 行
    let syncMedia = true;
    let autoSync = true;
    // `autoSyncMediaMinutes`：勾选 = 15 分钟，取消 = 0（对照 `profiles.py:722`）
    let periodicMediaSync = true;
    let networkTimeout = 60;
    let customSyncUrl = "";

    // 集合配置（`Preferences` 消息）：UI 不展示的字段也必须原样回写，
    // 所以每次都是 get → 改副本 → set（对照 Qt 的 `update_collection`）
    let prefs: Preferences | null = null;
    let rollover = 4;
    let learnAheadMinutes = 20;
    let timeLimitMinutes = 0;
    let showPlayButtons = true;
    let interruptAudio = true;
    let showRemainingCounts = true;
    let showIntervals = true;
    let renderLatex = false;
    let pasteImagesAsPng = false;
    let pasteStripsFormatting = false;
    let addToCurrentDeck = true;
    let defaultSearchText = "";
    let ignoreAccentsInSearch = false;
    let backupDaily = 12;
    let backupWeekly = 10;
    let backupMonthly = 9;
    let backupIntervalMins = 30;

    let languages: { name: string; code: string }[] = [];
    let status = "";
    let loading = true;

    const text = (key: string): string => translateRaw(key);

    /** 复选框的统一处理：从事件里取 `checked`（不依赖 `bind:` 的更新时机）。 */
    const onToggle =
        (save: (value: boolean) => void) =>
        (event: Event): void => {
            save((event.currentTarget as HTMLInputElement).checked);
        };

    /** 底部提示：哪些设置要重启才生效（对照 `preferences.py:413-415`）。 */
    function showRestartNotice(key: string): void {
        status = text(key);
    }

    function loadLanguages(): Promise<void> {
        return new Promise((resolve) => {
            bridgeCommand("languages", (result: unknown) => {
                if (Array.isArray(result)) {
                    languages = result as { name: string; code: string }[];
                }
                resolve();
            });
        });
    }

    // 集合配置：get → 改副本 → set（保留 UI 不展示的 4 个字段）
    function updatePrefs(mutate: (message: Preferences) => void): void {
        if (!prefs) {
            return;
        }
        const next = Preferences.fromBinary(prefs.toBinary());
        next.scheduling ??= new Preferences_Scheduling();
        mutate(next);
        prefs = next;
        setPreferences(next).catch((error: unknown) => {
            status = String(error);
        });
    }

    onMount(async () => {
        await setupI18n({
            modules: [
                ModuleName.PREFERENCES,
                ModuleName.ACTIONS,
                ModuleName.STUDYING,
                ModuleName.SCHEDULING,
                ModuleName.SYNC,
            ],
        });

        const meta = await Promise.all([
            getMeta("theme"),
            getMeta("defaultLang"),
            getMeta("uiScale"),
            getMeta("reduce_motion"),
            getMeta("minimalist_mode"),
            getMeta("hide_top_bar"),
            getMeta("top_bar_hide_mode"),
            getMeta("hide_bottom_bar"),
            getMeta("bottom_bar_hide_mode"),
            getMeta("spacebar_rates_card"),
            getMeta("check_for_updates"),
            getMeta("check_for_addon_updates"),
        ]);
        theme = meta[0] ?? 0;
        lang = meta[1] ?? "";
        uiScale = Math.round((meta[2] ?? 1) * 100);
        reduceMotion = meta[3] ?? true;
        minimalistMode = meta[4] ?? false;
        hideTopBar = meta[5] ?? false;
        topBarHideMode = meta[6] ?? 0;
        hideBottomBar = meta[7] ?? false;
        bottomBarHideMode = meta[8] ?? 0;
        spacebarRatesCard = meta[9] ?? true;
        checkForUpdates = meta[10] ?? true;
        checkForAddonUpdates = meta[11] ?? true;

        const profile = await Promise.all([
            getProfileConfig("syncMedia"),
            getProfileConfig("autoSync"),
            getProfileConfig("autoSyncMediaMinutes"),
            getProfileConfig("networkTimeout"),
            getProfileConfig("customSyncUrl"),
        ]);
        syncMedia = profile[0] ?? true;
        autoSync = profile[1] ?? true;
        periodicMediaSync = (profile[2] ?? 15) > 0;
        networkTimeout = profile[3] ?? 60;
        customSyncUrl = profile[4] ?? "";

        prefs = await getPreferences(new Empty({}));
        if (prefs.scheduling) {
            rollover = prefs.scheduling.rollover;
            learnAheadMinutes = Math.round(prefs.scheduling.learnAheadSecs / 60);
        }
        if (prefs.reviewing) {
            timeLimitMinutes = Math.round(prefs.reviewing.timeLimitSecs / 60);
            showPlayButtons = !prefs.reviewing.hideAudioPlayButtons;
            interruptAudio = prefs.reviewing.interruptAudioWhenAnswering;
            showRemainingCounts = prefs.reviewing.showRemainingDueCounts;
            showIntervals = prefs.reviewing.showIntervalsOnButtons;
        }
        if (prefs.editing) {
            renderLatex = prefs.editing.renderLatex;
            pasteImagesAsPng = prefs.editing.pasteImagesAsPng;
            pasteStripsFormatting = prefs.editing.pasteStripsFormatting;
            addToCurrentDeck = prefs.editing.addingDefaultsToCurrentDeck;
            defaultSearchText = prefs.editing.defaultSearchText;
            ignoreAccentsInSearch = prefs.editing.ignoreAccentsInSearch;
        }
        if (prefs.backups) {
            backupDaily = prefs.backups.daily;
            backupWeekly = prefs.backups.weekly;
            backupMonthly = prefs.backups.monthly;
            backupIntervalMins = prefs.backups.minimumIntervalMins;
        }

        await loadLanguages();
        loading = false;
    });

    function onThemeChanged(): void {
        // 宿主负责落库 + 广播到所有窗口（`theme:<0|1|2>`，见 `tauri/src/theme.rs`）
        bridgeCommand(`theme:${theme}`);
    }

    function onLanguageChanged(): void {
        setMeta("defaultLang", lang)
            .then(() => showRestartNotice("preferences-please-restart-anki-to-complete-language"))
            .catch((error: unknown) => {
                status = String(error);
            });
    }

    function onScaleChanged(): void {
        setMeta("uiScale", uiScale / 100)
            .then(() => showRestartNotice("preferences-changes-will-take-effect-when-you"))
            .catch((error: unknown) => {
                status = String(error);
            });
    }

    async function onCustomUrlChanged(): Promise<void> {
        // 对照 `ProfileManager.set_custom_sync_url`：改地址时把上一次被重定向到的
        // `currentSyncUrl` 一起清掉，否则会一直用旧端点。
        //
        // 两次写入必须**串行**：prefs21 的一个键一次整份读写（`Prefs21::set` 读改写整个
        // pickle dict），并发写会互相覆盖（实测 `Promise.all` 会把新地址冲掉）。
        try {
            await setProfileConfig("customSyncUrl", customSyncUrl);
            await setProfileConfig("currentSyncUrl", null);
        } catch (error: unknown) {
            status = String(error);
        }
    }

    function openHelp(): void {
        void helpPageLink({ page: HelpPageLinkRequest_HelpPage.PREFERENCES })
            .then((link) => openLink({ val: link.val }))
            .catch((error: unknown) => {
                status = String(error);
            });
    }

    function close(): void {
        bridgeCommand("close");
    }
</script>

<div class="prefs">
    <div class="tabs">
        {#each tabs as item (item.id)}
            <button
                class="tab"
                class:selected={tab === item.id}
                data-tab={item.id}
                on:click={() => (tab = item.id)}>{text(item.key)}</button
            >
        {/each}
    </div>

    <div class="body">
        {#if loading}
            <p>…</p>
        {:else if tab === "appearance"}
            <h3>{text("preferences-general")}</h3>
            <div class="row">
                <label for="prefs-lang">{text("preferences-language")}</label>
                <select id="prefs-lang" bind:value={lang} on:change={onLanguageChanged}>
                    {#each languages as item (item.code)}
                        <option value={item.code}>{item.name}</option>
                    {/each}
                </select>
            </div>
            <div class="row">
                <label for="prefs-scale">{text("preferences-user-interface-size")}</label>
                <span class="control">
                    <input
                        id="prefs-scale"
                        type="number"
                        min="100"
                        max="200"
                        step="5"
                        bind:value={uiScale}
                        on:change={onScaleChanged}
                    />
                    <span class="hint">%</span>
                </span>
            </div>

            <h3>{text("preferences-updates")}</h3>
            <div class="row">
                <label for="prefs-check-updates">{text("preferences-check-for-updates")}</label>
                <span class="control">
                    <Switch
                        id="prefs-check-updates"
                        bind:value={checkForUpdates}
                        on:input={onToggle((value) => void setMeta("check_for_updates", value))}
                    />
                    <span class="pending">⚠️ 外壳还没实现更新检查</span>
                </span>
            </div>
            <div class="row">
                <label for="prefs-check-addon-updates"
                    >{text("preferences-check-for-addon-updates")}</label
                >
                <Switch
                    id="prefs-check-addon-updates"
                    bind:value={checkForAddonUpdates}
                    on:input={onToggle((value) => void setMeta("check_for_addon_updates", value))}
                />
            </div>

            <h3>{text("preferences-user-interface")}</h3>
            <div class="row">
                <label for="prefs-theme">{text("preferences-theme")}</label>
                <select id="prefs-theme" bind:value={theme} on:change={onThemeChanged}>
                    <option value={0}>{text("preferences-theme-follow-system")}</option>
                    <option value={1}>{text("preferences-theme-light")}</option>
                    <option value={2}>{text("preferences-theme-dark")}</option>
                </select>
            </div>

            <h3>{text("preferences-distractions")}</h3>
            <div class="row">
                <label for="prefs-hide-top-bar"
                    >{text("preferences-hide-top-bar-during-review")}</label
                >
                <span class="control">
                    <Switch
                        id="prefs-hide-top-bar"
                        bind:value={hideTopBar}
                        on:input={onToggle((value) => void setMeta("hide_top_bar", value))}
                    />
                    <select
                        id="prefs-top-bar-mode"
                        bind:value={topBarHideMode}
                        on:change={() => void setMeta("top_bar_hide_mode", topBarHideMode)}
                    >
                        <option value={0}>{text("preferences-full-screen-only")}</option>
                        <option value={1}>{text("preferences-always")}</option>
                    </select>
                </span>
            </div>
            <div class="row">
                <label for="prefs-hide-bottom-bar"
                    >{text("preferences-hide-bottom-bar-during-review")}</label
                >
                <span class="control">
                    <Switch
                        id="prefs-hide-bottom-bar"
                        bind:value={hideBottomBar}
                        on:input={onToggle((value) => void setMeta("hide_bottom_bar", value))}
                    />
                    <select
                        id="prefs-bottom-bar-mode"
                        bind:value={bottomBarHideMode}
                        on:change={() => void setMeta("bottom_bar_hide_mode", bottomBarHideMode)}
                    >
                        <option value={0}>{text("preferences-full-screen-only")}</option>
                        <option value={1}>{text("preferences-always")}</option>
                    </select>
                </span>
            </div>
            <div class="row">
                <label for="prefs-reduce-motion">{text("preferences-reduce-motion")}</label>
                <Switch
                    id="prefs-reduce-motion"
                    bind:value={reduceMotion}
                    on:input={onToggle((value) => void setMeta("reduce_motion", value))}
                />
            </div>
            <div class="row">
                <label for="prefs-minimalist">{text("preferences-minimalist-mode")}</label>
                <Switch
                    id="prefs-minimalist"
                    bind:value={minimalistMode}
                    on:input={onToggle((value) => void setMeta("minimalist_mode", value))}
                />
            </div>
            <p class="note">⚠️「干扰项」这几项（隐藏上下工具栏、减少动画、极简模式）目前只落库，
                外壳还没消费（见 PARITY 8.3）。</p>
        {:else if tab === "review"}
            <h3>{text("preferences-scheduler")}</h3>
            <div class="row">
                <label for="prefs-rollover">{text("preferences-next-day-starts-at")}</label>
                <span class="control">
                    <input
                        id="prefs-rollover"
                        type="number"
                        min="0"
                        max="23"
                        bind:value={rollover}
                        on:change={() =>
                            updatePrefs((message) => {
                                message.scheduling!.rollover = rollover;
                            })}
                    />
                    <span class="hint">{text("preferences-hours-past-midnight")}</span>
                </span>
            </div>
            <div class="row">
                <label for="prefs-learn-ahead">{text("preferences-learn-ahead-limit")}</label>
                <span class="control">
                    <input
                        id="prefs-learn-ahead"
                        type="number"
                        min="0"
                        max="999"
                        bind:value={learnAheadMinutes}
                        on:change={() =>
                            updatePrefs((message) => {
                                message.scheduling!.learnAheadSecs = learnAheadMinutes * 60;
                            })}
                    />
                    <span class="hint">{text("preferences-mins")}</span>
                </span>
            </div>
            <div class="row">
                <label for="prefs-time-limit">{text("preferences-timebox-time-limit")}</label>
                <span class="control">
                    <input
                        id="prefs-time-limit"
                        type="number"
                        min="0"
                        max="9999"
                        bind:value={timeLimitMinutes}
                        on:change={() =>
                            updatePrefs((message) => {
                                message.reviewing!.timeLimitSecs = timeLimitMinutes * 60;
                            })}
                    />
                    <span class="hint">{text("preferences-mins")}</span>
                </span>
            </div>

            <h3>{text("preferences-review")}</h3>
            <div class="row">
                <label for="prefs-play-buttons"
                    >{text("preferences-show-play-buttons-on-cards-with")}</label
                >
                <Switch
                    id="prefs-play-buttons"
                    bind:value={showPlayButtons}
                    on:input={onToggle((value) =>
                        updatePrefs((message) => {
                            message.reviewing!.hideAudioPlayButtons = !value;
                        }))}
                />
            </div>
            <div class="row">
                <label for="prefs-interrupt"
                    >{text("preferences-interrupt-current-audio-when-answering")}</label
                >
                <Switch
                    id="prefs-interrupt"
                    bind:value={interruptAudio}
                    on:input={onToggle((value) =>
                        updatePrefs((message) => {
                            message.reviewing!.interruptAudioWhenAnswering = value;
                        }))}
                />
            </div>
            <div class="row">
                <label for="prefs-remaining">{text("preferences-show-remaining-card-count")}</label>
                <Switch
                    id="prefs-remaining"
                    bind:value={showRemainingCounts}
                    on:input={onToggle((value) =>
                        updatePrefs((message) => {
                            message.reviewing!.showRemainingDueCounts = value;
                        }))}
                />
            </div>
            <div class="row">
                <label for="prefs-intervals"
                    >{text("preferences-show-next-review-time-above-answer")}</label
                >
                <Switch
                    id="prefs-intervals"
                    bind:value={showIntervals}
                    on:input={onToggle((value) =>
                        updatePrefs((message) => {
                            message.reviewing!.showIntervalsOnButtons = value;
                        }))}
                />
            </div>
            <div class="row">
                <label for="prefs-spacebar">{text("preferences-spacebar-rates-card")}</label>
                <Switch
                    id="prefs-spacebar"
                    bind:value={spacebarRatesCard}
                    on:input={onToggle((value) => void setMeta("spacebar_rates_card", value))}
                />
            </div>
            <div class="row">
                <label for="prefs-latex"
                    >{text("preferences-generate-latex-images-automatically")}</label
                >
                <Switch
                    id="prefs-latex"
                    bind:value={renderLatex}
                    on:input={onToggle((value) =>
                        updatePrefs((message) => {
                            message.editing!.renderLatex = value;
                        }))}
                />
            </div>
            <p class="note">「空格评分」（`meta.spacebar_rates_card`）在复习界面的键盘答题里生效：
                答案面按空格 / 回车时，开启则用默认档评分，关闭则什么都不做（对照
                `reviewer.py:661`）。自定义答案键（`answer_keys`）的键是整数，现有 prefs21
                端点读不出来，暂时固定用 1–4。音频（重播 / 暂停 / ±5 秒 / 录音）与自动前进
                （`Shift+A`）的快捷键还没有对应的后端能力（PARITY 2.2 / 2.6）。</p>
        {:else if tab === "editing"}
            <h3>{text("preferences-editing")}</h3>
            <div class="row">
                <label for="prefs-paste-png"
                    >{text("preferences-paste-clipboard-images-as-png")}</label
                >
                <Switch
                    id="prefs-paste-png"
                    bind:value={pasteImagesAsPng}
                    on:input={onToggle((value) =>
                        updatePrefs((message) => {
                            message.editing!.pasteImagesAsPng = value;
                        }))}
                />
            </div>
            <div class="row">
                <label for="prefs-paste-formatting"
                    >{text("preferences-paste-without-shift-key-strips-formatting")}</label
                >
                <Switch
                    id="prefs-paste-formatting"
                    bind:value={pasteStripsFormatting}
                    on:input={onToggle((value) =>
                        updatePrefs((message) => {
                            message.editing!.pasteStripsFormatting = value;
                        }))}
                />
            </div>
            <div class="row">
                <label for="prefs-default-deck">{text("preferences-default-deck")}</label>
                <select
                    id="prefs-default-deck"
                    bind:value={addToCurrentDeck}
                    on:change={() =>
                        updatePrefs((message) => {
                            message.editing!.addingDefaultsToCurrentDeck = addToCurrentDeck;
                        })}
                >
                    <option value={true}
                        >{text("preferences-when-adding-default-to-current-deck")}</option
                    >
                    <option value={false}
                        >{text("preferences-change-deck-depending-on-note-type")}</option
                    >
                </select>
            </div>

            <h3>{text("preferences-browsing")}</h3>
            <div class="row">
                <label for="prefs-default-search">{text("preferences-default-search-text")}</label>
                <input
                    id="prefs-default-search"
                    type="text"
                    placeholder={text("preferences-default-search-text-example")}
                    bind:value={defaultSearchText}
                    on:change={() =>
                        updatePrefs((message) => {
                            message.editing!.defaultSearchText = defaultSearchText;
                        })}
                />
            </div>
            <div class="row">
                <label for="prefs-ignore-accents"
                    >{text("preferences-ignore-accents-in-search")}</label
                >
                <Switch
                    id="prefs-ignore-accents"
                    bind:value={ignoreAccentsInSearch}
                    on:input={onToggle((value) =>
                        updatePrefs((message) => {
                            message.editing!.ignoreAccentsInSearch = value;
                        }))}
                />
            </div>
        {:else if tab === "network"}
            <h3>{text("preferences-tab-synchronisation")}</h3>
            <div class="row">
                <label for="prefs-sync-media"
                    >{text("preferences-synchronize-audio-and-images-too")}</label
                >
                <Switch
                    id="prefs-sync-media"
                    bind:value={syncMedia}
                    on:input={onToggle((value) => void setProfileConfig("syncMedia", value))}
                />
            </div>
            <div class="row">
                <label for="prefs-auto-sync"
                    >{text("preferences-automatically-sync-on-profile-openclose")}</label
                >
                <Switch
                    id="prefs-auto-sync"
                    bind:value={autoSync}
                    on:input={onToggle((value) => void setProfileConfig("autoSync", value))}
                />
            </div>
            <div class="row">
                <label for="prefs-sync-media-minutes"
                    >{text("preferences-periodically-sync-media")}</label
                >
                <Switch
                    id="prefs-sync-media-minutes"
                    bind:value={periodicMediaSync}
                    on:input={onToggle((value) =>
                        void setProfileConfig("autoSyncMediaMinutes", value ? 15 : 0))}
                />
            </div>
            <div class="row">
                <label for="prefs-timeout">{text("preferences-network-timeout")}</label>
                <span class="control">
                    <input
                        id="prefs-timeout"
                        type="number"
                        min="30"
                        max="99999"
                        bind:value={networkTimeout}
                        on:change={() => void setProfileConfig("networkTimeout", networkTimeout)}
                    />
                    <span class="hint">{text("scheduling-seconds")}</span>
                </span>
            </div>
            <div class="row">
                <label for="prefs-custom-url">{text("preferences-custom-sync-url")}</label>
                <input
                    id="prefs-custom-url"
                    type="text"
                    placeholder={text("preferences-custom-sync-url-disclaimer")}
                    bind:value={customSyncUrl}
                    on:change={onCustomUrlChanged}
                />
            </div>
            <h3>{text("preferences-account")}</h3>
            <p class="note">{text("preferences-ankiweb-intro")}</p>
            <p class="note">⚠️ 登录 / 登出还在牌组列表工具栏的「同步」入口（快捷键 `Y`），
                见 PARITY 6.1。</p>
        {:else if tab === "backups"}
            <h3>{text("preferences-backups")}</h3>
            <p class="note">{text("preferences-backup-explanation")}</p>
            <div class="row">
                <label for="prefs-backup-interval"
                    >{text("preferences-minutes-between-backups")}</label
                >
                <input
                    id="prefs-backup-interval"
                    type="number"
                    min="5"
                    max="9999"
                    bind:value={backupIntervalMins}
                    on:change={() =>
                        updatePrefs((message) => {
                            message.backups!.minimumIntervalMins = backupIntervalMins;
                        })}
                />
            </div>
            <div class="row">
                <label for="prefs-backup-daily">{text("preferences-daily-backups")}</label>
                <input
                    id="prefs-backup-daily"
                    type="number"
                    min="0"
                    max="9999"
                    bind:value={backupDaily}
                    on:change={() =>
                        updatePrefs((message) => {
                            message.backups!.daily = backupDaily;
                        })}
                />
            </div>
            <div class="row">
                <label for="prefs-backup-weekly">{text("preferences-weekly-backups")}</label>
                <input
                    id="prefs-backup-weekly"
                    type="number"
                    min="0"
                    max="9999"
                    bind:value={backupWeekly}
                    on:change={() =>
                        updatePrefs((message) => {
                            message.backups!.weekly = backupWeekly;
                        })}
                />
            </div>
            <div class="row">
                <label for="prefs-backup-monthly">{text("preferences-monthly-backups")}</label>
                <input
                    id="prefs-backup-monthly"
                    type="number"
                    min="0"
                    max="9999"
                    bind:value={backupMonthly}
                    on:change={() =>
                        updatePrefs((message) => {
                            message.backups!.monthly = backupMonthly;
                        })}
                />
            </div>
            <h3>{text("preferences-note")}</h3>
            <p class="note">{text("preferences-media-is-not-backed-up")}</p>
        {:else if tab === "thirdparty"}
            <h3>{text("preferences-third-party-services")}</h3>
            <p class="note">{text("preferences-third-party-description")}</p>
            <h3>AnkiHub</h3>
            <p class="note">{text("preferences-ankihub-intro")}</p>
            <p class="note">⚠️ 外壳没有 addon 系统，AnkiHub 的登录 / 安装流程暂不可实现
                （PARITY 6.6）。</p>
        {:else if tab === "labs"}
            <h3>{text("preferences-experimental-features-tab")}</h3>
            <div class="lab-grid">
                <LabItem
                    title="Svelte note editor"
                    description="Replaces the legacy editor with a new Svelte-based implementation. May affect addon compatibility. You can also hold shift while opening the editor to temporarily toggle this option on or off."
                    key={ExperimentalFeatureFlag.SVELTE_EDITOR}
                    {labPerfs}
                ></LabItem>
            </div>
        {/if}
    </div>

    <div class="footer">
        <span class="status" id="prefs-status">{status}</span>
        <span class="spacer"></span>
        <button id="prefs-help" on:click={openHelp}>{text("actions-help")}</button>
        <button id="prefs-close" class="primary" on:click={close}>{text("actions-close")}</button>
    </div>
</div>

<style lang="scss">
    .prefs {
        display: flex;
        flex-direction: column;
        height: 100vh;
        padding: 1em;
        box-sizing: border-box;
        color: var(--fg);
        background: var(--canvas);
    }

    .tabs {
        display: flex;
        flex-wrap: wrap;
        gap: 0.25em;
        border-bottom: 1px solid var(--border);
        padding-bottom: 0.5em;
    }

    .tab {
        background: transparent;
        color: var(--fg);
        border: 1px solid transparent;
        border-radius: var(--border-radius);
        padding: 0.4em 0.8em;
        cursor: pointer;

        &.selected {
            border-color: var(--border);
            background: var(--canvas-elevated, rgba(128, 128, 128, 0.15));
            font-weight: bold;
        }
    }

    .body {
        flex: 1;
        overflow-y: auto;
        padding-top: 0.5em;
    }

    h3 {
        margin: 1em 0 0.5em;
        font-size: 1em;
        opacity: 0.8;
    }

    .row {
        display: flex;
        align-items: center;
        justify-content: space-between;
        gap: 1em;
        padding: 0.35em 0;
        max-width: 48em;
    }

    .row > label {
        flex: 1;
    }

    .control {
        display: inline-flex;
        align-items: center;
        gap: 0.4em;
    }

    input[type="text"],
    input[type="number"],
    select {
        min-width: 9em;
        padding: 0.25em 0.4em;
        color: var(--fg);
        background: var(--canvas);
        border: 1px solid var(--border);
        border-radius: var(--border-radius);
    }

    .hint,
    .pending {
        opacity: 0.6;
        font-size: 0.85em;
    }

    .note {
        max-width: 48em;
        opacity: 0.75;
        font-size: 0.9em;
        white-space: pre-line;
    }

    .lab-grid {
        display: grid;
        grid-template-columns: 1fr;
        gap: 1em;
        max-width: 48em;
    }

    .footer {
        display: flex;
        align-items: center;
        gap: 0.5em;
        border-top: 1px solid var(--border);
        padding-top: 0.5em;
    }

    .status {
        color: var(--fg-subtle);
        font-size: 0.9em;
    }

    .spacer {
        flex: 1;
    }

    .footer button {
        border: 1px solid var(--border);
        border-radius: var(--border-radius);
        background: var(--canvas);
        color: var(--fg);
        padding: 0.35em 0.9em;
        cursor: pointer;

        &.primary {
            font-weight: bold;
        }
    }
</style>
