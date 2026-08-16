import { invoke } from "@tauri-apps/api/core";
import type {
  ActivitySegment,
  SceneDay,
  HistorySlot,
  LlmReply,
  AiThread,
  AiMessage,
  AiSendResult,
  RoastRecord,
  RoastIndexEntry,
  AsrModelCandidate,
  AsrModelStatus,
  AssistantAction,
  AssistantConfig,
  AssistantUpdate,
  BrowserPairing,
  CategoryRule,
  ConfigSummary,
  DayStats,
  RangeStats,
  EventSummary,
  Health,
  ObserveStatus,
  OnboardingState,
  Permissions,
  PlatformInfo,
  SearchHit,
  SourcesUpdate,
  TimelineItem,
} from "./types";

export const api = {
  getHealth: () => invoke<Health>("get_health"),
  getPermissions: () => invoke<Permissions>("get_permissions"),
  getPlatformInfo: () => invoke<PlatformInfo>("get_platform_info"),
  searchText: (query: string, limit = 30) =>
    invoke<SearchHit[]>("search_text", { query, limit }),
  listEvents: (limit = 50) =>
    invoke<EventSummary[]>("list_events", { limit }),
  listTimeline: (opts: {
    limit?: number;
    kindContains?: string;
    appContains?: string;
    since?: string;
    until?: string;
  } = {}) =>
    invoke<TimelineItem[]>("list_timeline", {
      limit: opts.limit,
      kindContains: opts.kindContains,
      appContains: opts.appContains,
      since: opts.since,
      until: opts.until,
    }),
  getEventImageDataUrl: (eventId: string) =>
    invoke<string | null>("get_event_image_data_url", { eventId }),
  getEventMediaDataUrl: (eventId: string) =>
    invoke<string | null>("get_event_media_data_url", { eventId }),
  reindexSearch: () => invoke<number>("reindex_search"),
  getConfigSummary: () => invoke<ConfigSummary>("get_config_summary"),
  getBrowserPairing: () => invoke<BrowserPairing>("get_browser_pairing"),
  enableBrowserPairing: (rotate = false) =>
    invoke<BrowserPairing>("enable_browser_pairing", { rotate }),
  updateSourcesConfig: (update: SourcesUpdate) =>
    invoke<ConfigSummary>("update_sources_config", { update }),
  generateDaySummary: (day?: string) =>
    invoke<string>("generate_day_summary", { day: day ?? null }),
  activitySegments: (day: string) =>
    invoke<ActivitySegment[]>("activity_segments", { day }),
  activityScenes: (day: string) =>
    invoke<SceneDay>("activity_scenes", { day }),
  activityHistorySlots: (day: string) =>
    invoke<HistorySlot[]>("activity_history_slots", { day }),
  roastDay: (day: string, tone?: string) =>
    invoke<LlmReply>("roast_day", { day, tone: tone ?? null }),
  roastList: (day: string) => invoke<RoastRecord[]>("roast_list", { day }),
  roastIndex: () => invoke<RoastIndexEntry[]>("roast_index"),
  aiSend: (threadId: string | null, content: string) =>
    invoke<AiSendResult>("ai_send", { threadId, content }),
  aiThreadList: () => invoke<AiThread[]>("ai_thread_list"),
  aiThreadMessages: (threadId: string) =>
    invoke<AiMessage[]>("ai_thread_messages", { threadId }),
  aiThreadDelete: (threadId: string) =>
    invoke<void>("ai_thread_delete", { threadId }),
  llmTest: () => invoke<string>("llm_test"),
  llmListModels: () => invoke<string[]>("llm_list_models"),
  activityStats: (day: string, groupBy?: "app" | "site") =>
    invoke<DayStats>("activity_stats", { day, groupBy }),
  activityRange: (from: string, to: string, groupBy?: "app" | "site") =>
    invoke<RangeStats>("activity_range", { from, to, groupBy }),
  activityAddManualSegment: (opts: {
    startedAt: string;
    endedAt: string;
    appName: string;
    windowTitle?: string | null;
    category?: string | null;
    productivityLevel?: string | null;
  }) =>
    invoke<string>("activity_add_manual_segment", {
      startedAt: opts.startedAt,
      endedAt: opts.endedAt,
      appName: opts.appName,
      windowTitle: opts.windowTitle ?? null,
      category: opts.category ?? null,
      productivityLevel: opts.productivityLevel ?? null,
    }),
  activityDeleteSegment: (segId: string) =>
    invoke<void>("activity_delete_segment", { segId: segId }),
  activityListCategoryRules: () =>
    invoke<CategoryRule[]>("activity_list_category_rules"),
  activitySaveCategoryRules: (rules: CategoryRule[]) =>
    invoke<void>("activity_save_category_rules", { rules }),
  setPrivacyPaused: (paused: boolean) =>
    invoke<void>("set_privacy_paused", { paused }),
  observeStatus: () => invoke<ObserveStatus>("observe_status"),
  observeStart: () => invoke<ObserveStatus>("observe_start"),
  observeStop: () => invoke<ObserveStatus>("observe_stop"),
  openDataDir: () => invoke<void>("open_data_dir"),
  getBuildInfo: () => invoke<{ version: string; sha: string }>("get_build_info"),
  getOnboarding: () => invoke<OnboardingState>("get_onboarding"),
  setOnboardingStep: (step: number) =>
    invoke<OnboardingState>("set_onboarding_step", { step }),
  completeOnboarding: (launchObserve: boolean) =>
    invoke<OnboardingState>("complete_onboarding", { launchObserve }),
  skipOnboarding: () => invoke<OnboardingState>("skip_onboarding"),
  reopenOnboarding: () => invoke<OnboardingState>("reopen_onboarding"),
  setLaunchObserve: (enabled: boolean) =>
    invoke<void>("set_launch_observe", { enabled }),
  requestScreenPermission: () => invoke<boolean>("request_screen_permission"),
  refreshScreenPermission: () => invoke<boolean>("refresh_screen_permission"),
  requestMicrophonePermission: () => invoke<boolean>("request_microphone_permission"),
  openPrivacySettings: (kind: string) =>
    invoke<void>("open_privacy_settings", { kind }),
  checkAsrModelStatus: () => invoke<AsrModelStatus>("check_asr_model_status"),
  listLocalAsrModels: () => invoke<AsrModelCandidate[]>("list_local_asr_models"),
  useExistingAsrModel: (path: string, engine?: string) =>
    invoke<AsrModelStatus>("use_existing_asr_model", {
      input: { path, engine: engine ?? null },
    }),
  setAsrEnginePreference: (engine: string) =>
    invoke<AsrModelStatus>("set_asr_engine_preference", { engine }),
  setAsrModelsRoot: (modelsRoot: string) =>
    invoke<AsrModelStatus>("set_asr_models_root", { modelsRoot }),
  startAsrModelDownload: () => invoke<AsrModelStatus>("start_asr_model_download"),
  cancelAsrModelDownload: () => invoke<void>("cancel_asr_model_download"),
  assistantGetConfig: () => invoke<AssistantConfig>("assistant_get_config"),
  assistantUpdateConfig: (update: AssistantUpdate) =>
    invoke<AssistantConfig>("assistant_update_config", { update }),
  assistantRun: (action: AssistantAction, text: string, question?: string) =>
    invoke<string>("assistant_run", {
      action,
      text,
      question: question ?? null,
    }),
  assistantCancel: (id: string) => invoke<void>("assistant_cancel", { id }),
  requestAccessibilityPermission: () =>
    invoke<boolean>("request_accessibility_permission"),
  selectionPopupHide: () => invoke<void>("selection_popup_hide"),
  selectionPopupCurrent: () => invoke<string | null>("selection_popup_current"),
};
