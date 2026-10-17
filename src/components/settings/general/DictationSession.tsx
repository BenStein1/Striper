import React, { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { Mic, MicOff, Pause } from "lucide-react";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { ToggleSwitch } from "../../ui/ToggleSwitch";
import { useSettings } from "../../../hooks/useSettings";

type SessionState = "off" | "listening" | "paused";

type DictationSessionStatus = {
  state: SessionState;
  error: string | null;
};

const STYLES: Record<SessionState, string> = {
  off: "bg-mid-gray/20 text-text border-mid-gray/40 hover:bg-mid-gray/30",
  listening:
    "bg-green-600/20 text-green-700 dark:text-green-300 border-green-600/60 hover:bg-green-600/30",
  paused:
    "bg-amber-500/20 text-amber-700 dark:text-amber-300 border-amber-500/60 hover:bg-amber-500/30",
};

/**
 * The large Start dictation / End dictation control (PLAN.md §5a) with an
 * unmistakable Listening / Paused / Off indicator. Ctrl+Space does the same
 * thing when hands-free mode is on.
 */
export const DictationSession: React.FC = () => {
  const { t } = useTranslation();
  const { getSetting, updateSetting, isUpdating } = useSettings();
  const [status, setStatus] = useState<DictationSessionStatus>({
    state: "off",
    error: null,
  });
  const [busy, setBusy] = useState(false);

  const refresh = useCallback(async () => {
    try {
      setStatus(
        await invoke<DictationSessionStatus>("get_dictation_session_status"),
      );
    } catch (error) {
      toast.error(String(error));
    }
  }, []);

  useEffect(() => {
    void refresh();
    const unlisten = listen<DictationSessionStatus>(
      "dictation-session-state",
      (event) => setStatus(event.payload),
    );
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, [refresh]);

  const toggle = async () => {
    setBusy(true);
    try {
      setStatus(
        await invoke<DictationSessionStatus>("toggle_dictation_session"),
      );
    } catch (error) {
      toast.error(String(error));
    } finally {
      setBusy(false);
    }
  };

  const Icon =
    status.state === "listening"
      ? Mic
      : status.state === "paused"
        ? Pause
        : MicOff;
  const handsFree = getSetting("hands_free_shortcut") ?? false;

  return (
    <SettingsGroup
      title={t("settings.dictationSession.title")}
      description={t("settings.dictationSession.description")}
    >
      <div className="px-4 py-4 space-y-3">
        <button
          type="button"
          onClick={() => void toggle()}
          disabled={busy}
          aria-live="polite"
          className={`w-full flex items-center justify-center gap-4 rounded-xl border-2 px-6 py-6 text-2xl font-semibold transition-colors disabled:opacity-60 ${STYLES[status.state]}`}
        >
          <Icon className="w-9 h-9" aria-hidden="true" />
          <span>
            {status.state === "off"
              ? t("settings.dictationSession.start")
              : t("settings.dictationSession.end")}
          </span>
        </button>
        <p className="text-center text-lg font-medium" role="status">
          {t(`settings.dictationSession.state.${status.state}`)}
        </p>
        <p className="text-center text-sm text-mid-gray">
          {t(`settings.dictationSession.hint.${status.state}`)}
        </p>
        {status.error && (
          <p className="text-center text-sm text-red-500">{status.error}</p>
        )}
      </div>
      <ToggleSwitch
        checked={handsFree}
        onChange={(enabled) => updateSetting("hands_free_shortcut", enabled)}
        isUpdating={isUpdating("hands_free_shortcut")}
        label={t("settings.dictationSession.shortcut.label")}
        description={t("settings.dictationSession.shortcut.description")}
        descriptionMode="inline"
        grouped
      />
    </SettingsGroup>
  );
};
