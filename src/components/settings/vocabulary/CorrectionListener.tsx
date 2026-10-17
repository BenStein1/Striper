import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { ToggleSwitch } from "../../ui/ToggleSwitch";
import { Button } from "../../ui/Button";

type ListenerStatus = {
  installed: boolean;
  enabled: boolean;
  phase: string;
  error: string | null;
};

export function CorrectionListener() {
  const { t } = useTranslation();
  const [status, setStatus] = useState<ListenerStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const refresh = useCallback(async () => {
    try {
      setStatus(await invoke<ListenerStatus>("get_correction_listener_status"));
    } catch (error) {
      toast.error(String(error));
    }
  }, []);
  useEffect(() => {
    void refresh();
    const unlisten = listen<ListenerStatus>(
      "correction-listener-status",
      (event) => setStatus(event.payload),
    );
    return () => {
      void unlisten.then((stop) => stop());
    };
  }, [refresh]);

  const install = async () => {
    setBusy(true);
    try {
      await invoke("install_correction_listener");
      await refresh();
      toast.success(t("settings.vocabulary.listener.installed"));
    } catch (error) {
      toast.error(String(error));
    } finally {
      setBusy(false);
    }
  };
  const toggle = async (enabled: boolean) => {
    setBusy(true);
    try {
      await invoke("change_correction_listener_enabled_setting", { enabled });
      await refresh();
    } catch (error) {
      toast.error(String(error));
    } finally {
      setBusy(false);
    }
  };
  return (
    <SettingsGroup
      title={t("settings.vocabulary.listener.title")}
      description={t("settings.vocabulary.listener.description")}
    >
      <ToggleSwitch
        checked={status?.enabled ?? false}
        onChange={(enabled) => {
          void toggle(enabled);
        }}
        disabled={!status?.installed}
        isUpdating={busy}
        label={t("settings.vocabulary.listener.enable")}
        description={t("settings.vocabulary.listener.privacy")}
        descriptionMode="inline"
        grouped
      />
      <div className="px-4 py-3 space-y-3 text-sm">
        <p className="text-mid-gray">
          {t("settings.vocabulary.listener.instructions")}
        </p>
        <p role="status">
          {t("settings.vocabulary.listener.status", {
            state: t(
              `settings.vocabulary.listener.phases.${status?.phase || "off"}`,
            ),
          })}
        </p>
        {status?.error && (
          <p role="alert" className="text-red-500">
            {status.error}
          </p>
        )}
        {!status?.installed && (
          <Button
            variant="secondary"
            size="md"
            disabled={busy || status === null}
            onClick={() => {
              void install();
            }}
          >
            {t(
              busy
                ? "settings.vocabulary.listener.installing"
                : "settings.vocabulary.listener.install",
            )}
          </Button>
        )}
      </div>
    </SettingsGroup>
  );
}
