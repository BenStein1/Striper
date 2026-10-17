import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import { useTranslation } from "react-i18next";
import { toast } from "sonner";
import { SettingsGroup } from "../../ui/SettingsGroup";
import { Button } from "../../ui/Button";

type Capabilities = {
  os: string;
  session: string;
  desktop: string;
  focus_source: string;
  window_observable: boolean;
  input_backend: string;
  readback_required: boolean;
  listener_supported: boolean;
  tools: { name: string; installed: boolean }[];
  guidance: string[];
};

export function CorrectionCompatibility() {
  const { t } = useTranslation();
  const [report, setReport] = useState<Capabilities | null>(null);
  const [busy, setBusy] = useState(false);
  const refresh = useCallback(async () => {
    setBusy(true);
    try {
      setReport(await invoke<Capabilities>("get_correction_capabilities"));
    } catch (error) {
      toast.error(String(error));
    } finally {
      setBusy(false);
    }
  }, []);
  useEffect(() => {
    void refresh();
  }, [refresh]);
  const copy = async () => {
    if (!report) return;
    try {
      await writeText(JSON.stringify(report, null, 2));
      toast.success(t("settings.vocabulary.compatibility.copied"));
    } catch (error) {
      toast.error(String(error));
    }
  };
  const yesNo = (value: boolean) =>
    t(
      `settings.vocabulary.compatibility.${value ? "detected" : "notDetected"}`,
    );
  const rows = report
    ? [
        [
          t("settings.vocabulary.compatibility.desktop"),
          [report.os, report.session, report.desktop]
            .filter(Boolean)
            .join(" · "),
        ],
        [
          t("settings.vocabulary.compatibility.focus"),
          `${yesNo(report.window_observable)} (${report.focus_source})`,
        ],
        [t("settings.vocabulary.compatibility.input"), report.input_backend],
        [
          t("settings.vocabulary.compatibility.readback"),
          t(
            `settings.vocabulary.compatibility.${report.readback_required ? "required" : "disabled"}`,
          ),
        ],
        [
          t("settings.vocabulary.compatibility.wake"),
          t(
            `settings.vocabulary.compatibility.${report.listener_supported ? "available" : "unavailable"}`,
          ),
        ],
      ]
    : [];
  return (
    <SettingsGroup
      title={t("settings.vocabulary.compatibility.title")}
      description={t("settings.vocabulary.compatibility.description")}
    >
      <div className="p-4 space-y-3 text-sm">
        {report && (
          <>
            <dl className="grid grid-cols-[minmax(0,1fr)_minmax(0,1.4fr)] gap-x-4 gap-y-2">
              {rows.map(([label, value]) => (
                <div key={label} className="contents">
                  <dt className="text-mid-gray">{label}</dt>
                  <dd className="break-words">{value}</dd>
                </div>
              ))}
            </dl>
            {report.tools.length > 0 && (
              <div className="flex flex-wrap gap-2">
                {report.tools.map((tool) => (
                  <span
                    key={tool.name}
                    className={`rounded px-2 py-1 text-xs ${tool.installed ? "bg-logo-primary/10" : "bg-mid-gray/10 text-mid-gray"}`}
                  >
                    {t("settings.vocabulary.compatibility.toolStatus", {
                      tool: tool.name,
                      status: yesNo(tool.installed),
                    })}
                  </span>
                ))}
              </div>
            )}
            <ul className="list-disc ps-5 space-y-2 text-mid-gray">
              {report.guidance.map((key) => (
                <li key={key}>
                  {t(`settings.vocabulary.compatibility.guidance.${key}`)}
                </li>
              ))}
            </ul>
          </>
        )}
        <div className="flex flex-wrap gap-2">
          <Button
            variant="secondary"
            size="md"
            disabled={busy}
            onClick={() => {
              void refresh();
            }}
          >
            {t("settings.vocabulary.compatibility.refresh")}
          </Button>
          <Button
            variant="secondary"
            size="md"
            disabled={busy || !report}
            onClick={() => {
              void copy();
            }}
          >
            {t("settings.vocabulary.compatibility.copy")}
          </Button>
        </div>
      </div>
    </SettingsGroup>
  );
}
