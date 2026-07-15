import { invoke } from "@tauri-apps/api/core";
import type {
  ConfigSummary,
  EventSummary,
  Health,
  ObserveStatus,
  OnboardingState,
  Permissions,
  SearchHit,
} from "./types";

export const api = {
  getHealth: () => invoke<Health>("get_health"),
  getPermissions: () => invoke<Permissions>("get_permissions"),
  searchText: (query: string, limit = 30) =>
    invoke<SearchHit[]>("search_text", { query, limit }),
  listEvents: (limit = 50) =>
    invoke<EventSummary[]>("list_events", { limit }),
  reindexSearch: () => invoke<number>("reindex_search"),
  getConfigSummary: () => invoke<ConfigSummary>("get_config_summary"),
  setPrivacyPaused: (paused: boolean) =>
    invoke<void>("set_privacy_paused", { paused }),
  observeStatus: () => invoke<ObserveStatus>("observe_status"),
  observeStart: () => invoke<ObserveStatus>("observe_start"),
  observeStop: () => invoke<ObserveStatus>("observe_stop"),
  openDataDir: () => invoke<void>("open_data_dir"),
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
  openPrivacySettings: (kind: string) =>
    invoke<void>("open_privacy_settings", { kind }),
};
