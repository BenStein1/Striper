import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { useTranslation } from "react-i18next";
import { commands, type AvailableAccelerators } from "@/bindings";
import { useSettingsStore } from "@/stores/settingsStore";

/** Capability label, deliberately not a claim about the backend currently loaded. */
export const ModelComputeBadge = () => {
  const { t } = useTranslation();
  return (
    <span
      className="text-xs text-text/60"
      title={t("modelSelector.compute.capabilityDescription")}
    >
      {t("modelSelector.compute.capability")}
    </span>
  );
};

export const ModelComputeSummary = () => {
  const { t } = useTranslation();
  const preference = useSettingsStore(
    (state) => state.settings?.transcribe_accelerator ?? "auto",
  );
  const selectedDevice = useSettingsStore(
    (state) => state.settings?.transcribe_gpu_device,
  );
  const [available, setAvailable] = useState<AvailableAccelerators | null>(
    null,
  );
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let disposed = false;
    let generation = 0;
    const refresh = async () => {
      const request = ++generation;
      try {
        const result = await commands.getAvailableAccelerators();
        if (!disposed && request === generation) {
          setAvailable(result);
          setFailed(false);
        }
      } catch {
        if (!disposed && request === generation) {
          setAvailable(null);
          setFailed(true);
        }
      }
    };
    const unlisten = listen("model-state-changed", () => {
      void refresh();
    });
    void refresh();
    // Re-check when returning from hardware settings or a desktop test.
    window.addEventListener("focus", refresh);
    return () => {
      disposed = true;
      window.removeEventListener("focus", refresh);
      void unlisten.then((stop) => stop()).catch(() => {});
    };
  }, []);

  const preferenceLabel =
    preference === "cpu"
      ? "CPU"
      : preference === "gpu"
        ? (available?.gpu_devices.find((device) => device.id === selectedDevice)
            ?.name ?? "GPU")
        : t("settings.advanced.acceleration.gpuDevice.auto");

  return (
    <div className="space-y-1 text-xs text-text/60">
      <p>
        {t("modelSelector.compute.preference", { preference: preferenceLabel })}
      </p>
      {available ? (
        <p>
          {available.gpu_devices.length > 0
            ? t("modelSelector.compute.detected", {
                devices: available.gpu_devices
                  .map((device) => `${device.name} · ${device.backend}`)
                  .join(", "),
              })
            : t("modelSelector.compute.noGpu")}
        </p>
      ) : (
        <p>
          {t(
            failed
              ? "modelSelector.compute.unavailable"
              : "modelSelector.compute.checking",
          )}
        </p>
      )}
      {available && (
        <p className="font-medium text-text/80" role="status">
          {available.active_backend && available.active_on_gpu !== null
            ? t("modelSelector.compute.active", {
                kind: available.active_on_gpu ? "GPU" : "CPU",
                backend: available.active_backend,
                device: available.active_device || available.active_backend,
              })
            : t("modelSelector.compute.notLoaded")}
        </p>
      )}
      <p>{t("modelSelector.compute.guidance")}</p>
    </div>
  );
};
