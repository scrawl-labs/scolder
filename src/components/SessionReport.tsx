import { useState } from "react";
import { Utterance } from "../types";

interface SessionReportProps {
  utterances: Utterance[];
  reportPath?: string;
  isVisible: boolean;
  markdownContent?: string;
}

type ReportTab = "corrections" | "vocabulary";

export function SessionReport({
  utterances,
  reportPath,
  isVisible,
  markdownContent,
}: SessionReportProps) {
  const [activeTab, setActiveTab] = useState<ReportTab>("corrections");

  if (!isVisible) return null;

  // 구조화된 발화 기록(JSON)이 있으면 항상 이걸 우선 사용 — 라이브 리포트와 동일한 카드 UI.
  // markdownContent는 JSON 사이드카가 없는 옛날 세션에 대한 폴백일 때만 사용.
  if (utterances.length === 0 && markdownContent) {
    return (
      <pre className="text-xs text-zinc-500 dark:text-zinc-400 font-mono leading-relaxed whitespace-pre-wrap">
        {markdownContent}
      </pre>
    );
  }

  if (utterances.length === 0) {
    return (
      <div className="flex flex-col items-center justify-center h-full gap-2 select-none">
        <p className="text-xs text-zinc-400 dark:text-zinc-600 text-center leading-relaxed">
          No speech was captured in this session.
        </p>
      </div>
    );
  }

  const errorCount = utterances.filter((u) => u.feedback.has_error).length;
  const vocabItems = [
    ...new Set(
      utterances
        .filter((u) => u.feedback.idiom_or_vocab)
        .map((u) => u.feedback.idiom_or_vocab),
    ),
  ];
  const corrections = utterances.filter((u) => u.feedback.has_error);

  return (
    <div className="space-y-6">
      {/* 요약 통계 */}
      <div>
        <h3 className="text-xs font-medium text-zinc-500 dark:text-zinc-400 uppercase tracking-wider mb-3">
          Overview
        </h3>
        <div className="grid grid-cols-3 gap-3">
          {[
            {
              label: "Utterances",
              value: utterances.length,
              color: "text-zinc-800 dark:text-zinc-100",
            },
            {
              label: "Corrections",
              value: errorCount,
              color: "text-red-600 dark:text-red-400",
            },
            {
              label: "Vocab",
              value: vocabItems.length,
              color: "text-zinc-800 dark:text-zinc-100",
            },
          ].map(({ label, value, color }) => (
            <div
              key={label}
              className="min-w-0 border border-zinc-100 dark:border-zinc-800 rounded-lg p-2.5"
            >
              <div className={`text-xl font-semibold tabular-nums ${color}`}>
                {value}
              </div>
              <div className="text-[11px] leading-tight text-zinc-400 dark:text-zinc-500 mt-0.5">
                {label}
              </div>
            </div>
          ))}
        </div>
      </div>

      {/* 탭 전환: Corrections / Vocabulary */}
      <div>
        <div className="flex gap-1 border-b border-zinc-100 dark:border-zinc-800 mb-4">
          {(
            [
              { key: "corrections", label: "Corrections" },
              { key: "vocabulary", label: "Vocabulary" },
            ] as const
          ).map(({ key, label }) => (
            <button
              key={key}
              onClick={() => setActiveTab(key)}
              className={`px-3 py-2 text-xs font-medium border-b-2 -mb-px transition-colors cursor-pointer ${
                activeTab === key
                  ? "border-zinc-900 dark:border-white text-zinc-900 dark:text-white"
                  : "border-transparent text-zinc-400 dark:text-zinc-500 hover:text-zinc-600 dark:hover:text-zinc-300"
              }`}
            >
              {label}
            </button>
          ))}
        </div>

        {activeTab === "corrections" ? (
          corrections.length > 0 ? (
            <div className="space-y-3">
              {corrections.map((u, idx) => (
                <div
                  key={idx}
                  className="border-l-2 border-red-200 dark:border-red-800 pl-3"
                >
                  <div className="text-xs text-zinc-400 dark:text-zinc-500 font-mono tabular-nums mb-1">
                    {u.timestamp}
                  </div>
                  <p className="text-xs text-zinc-400 dark:text-zinc-500 mb-1 break-words">
                    "{u.feedback.original}"
                  </p>
                  {u.feedback.corrected && (
                    <p className="text-sm text-zinc-800 dark:text-zinc-200 font-medium break-words">
                      "{u.feedback.corrected}"
                    </p>
                  )}
                  {u.feedback.explanation && (
                    <p className="text-xs text-red-600/80 dark:text-red-400/80 mt-1 break-words">
                      {u.feedback.explanation}
                    </p>
                  )}
                  {u.feedback.better_expression && (
                    <p className="text-xs text-zinc-500 dark:text-zinc-400 mt-1 break-words">
                      {u.feedback.better_expression}
                    </p>
                  )}
                </div>
              ))}
            </div>
          ) : (
            <p className="text-xs text-zinc-400 dark:text-zinc-600 text-center py-6">
              No corrections needed — nice work!
            </p>
          )
        ) : vocabItems.length > 0 ? (
          <div className="space-y-3">
            {vocabItems.map((vocab, idx) => {
              // 백엔드가 "표현: 뜻" 형식으로 내려주는 문자열을 상하로 분리해서
              // 표현은 굵게, 뜻은 아래 작은 글씨로 — 한 줄로 붙어 있으면 읽기 힘들다는 피드백
              const text = vocab ?? "";
              const separatorIndex = text.indexOf(":");
              const term =
                separatorIndex === -1 ? text : text.slice(0, separatorIndex).trim();
              const meaning =
                separatorIndex === -1 ? "" : text.slice(separatorIndex + 1).trim();

              return (
                <div
                  key={idx}
                  className="py-1.5 border-b border-zinc-50 dark:border-zinc-800/50 last:border-0"
                >
                  <div className="text-sm font-medium text-zinc-800 dark:text-zinc-200 break-words">
                    {term}
                  </div>
                  {meaning && (
                    <div className="text-xs text-zinc-500 dark:text-zinc-400 mt-0.5 break-words">
                      {meaning}
                    </div>
                  )}
                </div>
              );
            })}
          </div>
        ) : (
          <p className="text-xs text-zinc-400 dark:text-zinc-600 text-center py-6">
            No new vocabulary this session.
          </p>
        )}
      </div>

      {reportPath && (
        <p className="text-xs text-zinc-400 dark:text-zinc-600 font-mono truncate pt-2 border-t border-zinc-100 dark:border-zinc-800">
          {reportPath}
        </p>
      )}
    </div>
  );
}
