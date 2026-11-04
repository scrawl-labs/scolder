import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import {
  checkMicrophonePermission,
  requestMicrophonePermission,
} from "tauri-plugin-macos-permissions-api";
import {
  RawUtterance,
  SessionProgress,
  SetupProgress,
  Stats,
  Utterance,
} from "./types";
import { RecordingStatus } from "./components/RecordingStatus";
import { TranscriptView } from "./components/TranscriptView";
import { SessionReport } from "./components/SessionReport";
import { SessionLog } from "./components/SessionLog";
import { SessionList, SessionInfo } from "./components/SessionList";
import { UploadModal } from "./components/UploadModal";
import {
  SunIcon,
  MoonIcon,
  StopIcon,
  MicrophoneIcon,
  XMarkIcon,
  DocumentTextIcon,
  ArrowPathIcon,
} from "@heroicons/react/20/solid";
import "./App.css";

export function App() {
  const [isRecording, setIsRecording] = useState(false);
  // 마이크 권한 응답 대기 등 start_session 진행 중 - 중복 클릭 방지
  const [isStarting, setIsStarting] = useState(false);
  const [sessionStartTime, setSessionStartTime] = useState<Date | null>(null);
  const [liveTranscript, setLiveTranscript] = useState<RawUtterance[]>([]);
  const [isGeneratingReport, setIsGeneratingReport] = useState(false);
  const [utterances, setUtterances] = useState<Utterance[]>([]);
  const [reportPath, setReportPath] = useState<string>("");
  const [showReport, setShowReport] = useState(false);
  const [showTranscript, setShowTranscript] = useState(false);
  const [loadedTranscript, setLoadedTranscript] = useState<string>("");
  const [errorMsg, setErrorMsg] = useState("");
  const [selectedSession, setSelectedSession] = useState<SessionInfo | null>(
    null,
  );
  const [selectedSessionMarkdown, setSelectedSessionMarkdown] =
    useState<string>("");
  const [sessionRefreshKey, setSessionRefreshKey] = useState(0);
  const [stats, setStats] = useState<Stats | null>(null);
  const [setupProgress, setSetupProgress] = useState<SetupProgress | null>(
    null,
  );
  const [isDark, setIsDark] = useState(
    () => window.matchMedia("(prefers-color-scheme: dark)").matches,
  );
  const [showUploadModal, setShowUploadModal] = useState(false);
  const [uploadProcessing, setUploadProcessing] = useState(false);
  const [uploadProgress, setUploadProgress] = useState<SessionProgress | null>(
    null,
  );
  const [uploadError, setUploadError] = useState("");

  useEffect(() => {
    document.documentElement.classList.toggle("dark", isDark);
  }, [isDark]);

  // 첫 실행 설치(Whisper 모델 + Ollama 런타임/서버 + LLM 모델) - 비개발자도
  // 터미널 없이 쓸 수 있도록 앱이 알아서 처리하고, 진행 상황을 "setup-progress"
  // 이벤트로 받아 화면에 표시. 이미 준비돼 있으면 커맨드가 거의 즉시 끝나서
  // 화면이 잠깐 스치듯 지나감.
  useEffect(() => {
    const p = listen<SetupProgress>("setup-progress", (e) =>
      setSetupProgress(e.payload),
    );
    invoke("run_setup").catch((err) => setErrorMsg(String(err)));
    return () => {
      p.then((u) => u());
    };
  }, []);

  // idle 화면 통계 대시보드 - 세션이 새로 생기거나 지워질 때마다(sessionRefreshKey) 재계산
  useEffect(() => {
    invoke<Stats>("get_stats")
      .then(setStats)
      .catch(() => {});
  }, [sessionRefreshKey]);

  useEffect(() => {
    const id = setInterval(async () => {
      try {
        const recording = await invoke<boolean>("get_recording_status");
        setIsRecording(recording);
        if (recording)
          setLiveTranscript(await invoke<RawUtterance[]>("get_transcript"));
      } catch {}
    }, 2000);
    return () => clearInterval(id);
  }, []);

  // 업로드 세션 처리 진행 상황 (디코딩 → 전사 → 피드백 생성) - 업로드 모달에서만 사용
  useEffect(() => {
    const p = listen<SessionProgress>("session-progress", (e) =>
      setUploadProgress(e.payload),
    );
    return () => {
      p.then((u) => u());
    };
  }, []);

  const handleStart = async () => {
    setErrorMsg("");
    setShowReport(false);
    setSelectedSession(null);
    setSelectedSessionMarkdown("");
    setLiveTranscript([]);
    setShowTranscript(false);
    setLoadedTranscript("");
    setUtterances([]);
    setReportPath("");
    setIsStarting(true);
    try {
      // cpal의 저수준 CoreAudio 호출만으로는 macOS 마이크 권한(TCC) 대화상자가
      // 아예 안 뜨는 경우가 있어, AVFoundation 기반 API로 명시적으로 요청한다.
      // 그렇지 않으면 시스템 설정에 앱 자체가 나타나지 않고 조용히 마이크가 막힘.
      //
      // 이 체크 결과로 세션 시작 자체를 막지는 않는다 - `tauri dev`로 띄운
      // 번들 안 된 실행파일은 자체 Info.plist/번들 아이덴티티가 없어 이 체크가
      // 항상 false를 줄 수 있음. 실제 마이크 캡처 성공 여부는 백엔드(cpal)가
      // 판단해서 실패 시 별도 에러로 알려준다 (audio.rs 참고).
      if (!(await checkMicrophonePermission())) {
        await requestMicrophonePermission();
      }

      // start_session은 마이크 권한 프롬프트 응답까지 기다리므로, 타이머는 실제 캡처가
      // 시작된 뒤에 켠다 (프롬프트 대기 시간이 녹음 시간에 섞이지 않게)
      await invoke("start_session");
      setSessionStartTime(new Date());
      setIsRecording(true);
    } catch (err) {
      setErrorMsg(String(err));
      setIsRecording(false);
      setSessionStartTime(null);
    } finally {
      setIsStarting(false);
    }
  };

  const handleStop = async () => {
    setIsGeneratingReport(true);
    try {
      // 녹음 중엔 LLM을 호출하지 않았으므로, 여기서 전문을 바탕으로 리포트를 생성한다.
      // 발화 수만큼 LLM 왕복이 필요해 몇 초 걸릴 수 있음 - isGeneratingReport로 로딩 표시.
      const path = await invoke<string>("stop_session");
      setIsRecording(false);
      setReportPath(path);
      setShowReport(true);
      setShowTranscript(false);
      setUtterances(await invoke<Utterance[]>("get_utterances"));
      setSessionRefreshKey((k) => k + 1);

      // 방금 끝난 세션을 "선택된 세션"으로 지정 - 헤더/사이드바가 "Ready"로
      // 되돌아가지 않고 방금 만든 세션을 가리키도록 함
      if (path) {
        setSelectedSession(await invoke<SessionInfo>("get_session_info", { path }));
      }
    } catch (err) {
      setErrorMsg(String(err));
    } finally {
      setIsGeneratingReport(false);
    }
  };

  // UploadModal의 드래그앤드롭 리스너 useEffect가 이 함수 참조([onUpload])에
  // 의존하므로, useCallback 없이 매 App 렌더마다 새 함수를 만들면 리스너가
  // 계속 해제/재구독된다. invoke와 setter들은 안정적이라 deps는 빈 배열로 둔다.
  const handleUploadFile = useCallback(async (path: string) => {
    setUploadError("");
    setUploadProcessing(true);
    setUploadProgress(null);
    try {
      const path_ = await invoke<string>("start_session_from_file", {
        path,
      });
      setUploadProcessing(false);
      setShowUploadModal(false);
      setShowReport(false);
      setSelectedSession(null);
      setSelectedSessionMarkdown("");
      setShowTranscript(false);
      setLoadedTranscript("");
      setReportPath(path_);
      setUtterances(await invoke<Utterance[]>("get_utterances"));
      setShowReport(true);
      setSessionRefreshKey((k) => k + 1);
      if (path_) {
        setSelectedSession(
          await invoke<SessionInfo>("get_session_info", { path: path_ }),
        );
      }
    } catch (err) {
      setUploadProcessing(false);
      setUploadError(String(err));
    }
  }, []);

  const handleToggleTranscript = async () => {
    if (!showTranscript && !isRecording) {
      const path = selectedSession?.path || reportPath;
      if (path) {
        try {
          setLoadedTranscript(
            await invoke<string>("read_session_transcript", { path }),
          );
        } catch (err) {
          setErrorMsg(String(err));
        }
      }
    }
    setShowTranscript((v) => !v);
  };

  const handleSelectSession = async (session: SessionInfo) => {
    setSelectedSession(session);
    setShowReport(false);
    setUtterances([]);
    setSelectedSessionMarkdown("");
    setShowTranscript(false);
    setLoadedTranscript("");
    try {
      // JSON 사이드카 존재 여부로 분기 - 발화 0건인 최신 세션과
      // JSON 자체가 없는 옛날 세션을 구분해야 함 (둘 다 배열 길이만으로는 구분 불가)
      const hasJson = await invoke<boolean>("session_has_utterances_json", {
        path: session.path,
      });

      if (hasJson) {
        // 구조화된 발화 기록(JSON) - 라이브 리포트와 동일한 카드/표 UI로 보여줌
        setUtterances(
          await invoke<Utterance[]>("read_session_utterances", {
            path: session.path,
          }),
        );
      } else {
        // JSON 사이드카가 없는 옛날 세션 → 마크다운 원본으로 폴백
        const content = await invoke<string>("read_session", {
          path: session.path,
        });
        setSelectedSessionMarkdown(content);
      }
      setShowReport(true);
    } catch (err) {
      setErrorMsg(String(err));
    }
  };

  const handleRenameSession = async (
    session: SessionInfo,
    newTitle: string,
  ) => {
    try {
      await invoke("rename_session", { path: session.path, newTitle });
      setSessionRefreshKey((k) => k + 1);
      const trimmed = newTitle.trim();
      if (trimmed && selectedSession?.path === session.path) {
        setSelectedSession({ ...selectedSession, title: trimmed });
      }
    } catch (err) {
      setErrorMsg(String(err));
    }
  };

  const handleDeleteSession = async (session: SessionInfo) => {
    try {
      await invoke("delete_session", { path: session.path });
      setSessionRefreshKey((k) => k + 1);
      if (selectedSession?.path === session.path) {
        setSelectedSession(null);
        setShowReport(false);
        setUtterances([]);
        setSelectedSessionMarkdown("");
      }
    } catch (err) {
      setErrorMsg(String(err));
    }
  };

  const isSetupReady = setupProgress?.stage === "ready";
  const canViewTranscript = isRecording || showReport;
  const newSessionDisabled =
    isRecording || isStarting || isGeneratingReport || !isSetupReady;
  const rightPanelTitle = showTranscript
    ? "Transcript"
    : isGeneratingReport
      ? "Report"
      : isRecording
        ? "Recording"
        : showReport
          ? (selectedSession?.title ?? "Report")
          : "Details";

  return (
    <div className="flex h-screen overflow-hidden bg-white dark:bg-zinc-900 font-sans antialiased">
      {/* ── 첫 실행 설치 진행 화면 - 준비될 때까지 전체 화면을 덮음 ── */}
      {!isSetupReady && (
        <div className="fixed inset-0 z-50 flex flex-col items-center justify-center gap-4 bg-white dark:bg-zinc-900">
          <ArrowPathIcon className="w-6 h-6 text-zinc-400 dark:text-zinc-600 animate-spin" />
          <p className="text-sm text-zinc-600 dark:text-zinc-400">
            {setupProgress?.message ?? "Starting up…"}
          </p>
          {setupProgress && setupProgress.percent >= 0 && (
            <div className="w-64">
              <div className="h-1.5 bg-zinc-100 dark:bg-zinc-800 rounded-full overflow-hidden">
                <div
                  className="h-full bg-zinc-900 dark:bg-white transition-[width] duration-300"
                  style={{ width: `${setupProgress.percent}%` }}
                />
              </div>
              <p className="text-xs text-zinc-400 dark:text-zinc-600 text-center mt-2 tabular-nums">
                {Math.round(setupProgress.percent)}%
              </p>
            </div>
          )}
        </div>
      )}

      {/* ── 다크 사이드바 ────────────────── */}
      <SessionList
        selectedSession={selectedSession}
        onSelectSession={handleSelectSession}
        onRenameSession={handleRenameSession}
        onDeleteSession={handleDeleteSession}
        onNewSession={handleStart}
        onUploadClick={() => setShowUploadModal(true)}
        newSessionDisabled={newSessionDisabled}
        refreshKey={sessionRefreshKey}
      />

      {showUploadModal && (
        <UploadModal
          onClose={() => {
            // 처리 중에도 닫기를 허용한다 - 업로드(invoke)는 useCallback([])이라
            // 모달이 닫혀도 백그라운드에서 계속 진행되고, 완료 시 sessionRefreshKey/
            // setShowReport 갱신으로 결과가 반영된다. 실제 취소는 별도 후속 작업.
            setShowUploadModal(false);
          }}
          onUpload={handleUploadFile}
          processing={uploadProcessing}
          progress={uploadProgress}
          error={uploadError}
        />
      )}

      {/* ── 메인 영역 ──────────────────────── */}
      <div className="flex flex-col flex-1 overflow-hidden">
        {/* 상단 툴바 */}
        <header className="flex items-center justify-between h-16 px-6 border-b border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 shrink-0">
          <div>
            <h1 className="text-base font-semibold text-zinc-900 dark:text-zinc-100">
              {isRecording
                ? "Session in progress"
                : selectedSession
                  ? selectedSession.title
                  : showReport
                    ? "Report"
                    : "Ready"}
            </h1>
            <p className="text-xs text-zinc-400 dark:text-zinc-500 mt-0.5">
              {isRecording
                ? "Speak naturally — your report is generated when you stop"
                : "Select a session, or click New Session to start recording"}
            </p>
          </div>
          <div className="flex items-center gap-2">
            <RecordingStatus
              isRecording={isRecording}
              sessionStartTime={sessionStartTime}
            />
            <div className="flex items-center gap-1.5">
              <span
                className={`w-1.5 h-1.5 rounded-full shrink-0 ${
                  isSetupReady ? "bg-emerald-500" : "bg-amber-500"
                }`}
              />
              <span className="text-xs text-zinc-500 dark:text-zinc-400">
                {isSetupReady ? "Model ready" : "Setting up…"}
              </span>
            </div>
            <button
              onClick={() => setIsDark((d) => !d)}
              className="p-1.5 rounded-md text-zinc-400 hover:text-zinc-600 dark:hover:text-zinc-300 hover:bg-zinc-100 dark:hover:bg-zinc-800 transition-colors"
            >
              {isDark ? (
                <SunIcon className="w-4 h-4" />
              ) : (
                <MoonIcon className="w-4 h-4" />
              )}
            </button>
          </div>
        </header>

        <div className="flex flex-1 overflow-hidden">
          {/* 중앙 컨텐츠 */}
          <main className="flex flex-col flex-1 min-w-0 min-h-0 overflow-y-auto px-6 py-6 bg-zinc-50 dark:bg-zinc-950">
            {/* 에러 배너 */}
            {errorMsg && (
              <div className="flex items-start gap-3 mb-5 p-3.5 bg-red-50 dark:bg-red-950/50 border border-red-200 dark:border-red-800/50 rounded-lg">
                <p className="text-xs text-red-700 dark:text-red-400 flex-1 whitespace-pre-wrap font-sans">
                  {errorMsg}
                </p>
                <button
                  onClick={() => setErrorMsg("")}
                  className="text-red-400 hover:text-red-600 dark:hover:text-red-300 shrink-0"
                >
                  <XMarkIcon className="w-3.5 h-3.5" />
                </button>
              </div>
            )}

            {/* 녹음 중/생성 중에만 상태 카드 표시 - Start 버튼은 사이드바의 New Session으로 이동 */}
            {isRecording || isGeneratingReport ? (
              <div className="bg-white dark:bg-zinc-900 border border-zinc-200 dark:border-zinc-800 rounded-xl">
                <div className="px-6 py-5 border-b border-zinc-100 dark:border-zinc-800">
                  <h2 className="text-sm font-medium text-zinc-900 dark:text-zinc-100">
                    Recording
                  </h2>
                  <p className="text-xs text-zinc-400 dark:text-zinc-500 mt-0.5">
                    Your full report is generated when you stop
                  </p>
                </div>
                <div className="px-6 py-6">
                  {/* 상태 표시줄 */}
                  <div className="flex items-center gap-3 mb-6 pb-5 border-b border-zinc-100 dark:border-zinc-800">
                    <div
                      className={`w-2.5 h-2.5 rounded-full shrink-0 ${
                        isRecording
                          ? "bg-red-500 animate-pulse"
                          : "bg-zinc-200 dark:bg-zinc-700"
                      }`}
                    />
                    <span className="text-sm text-zinc-600 dark:text-zinc-400">
                      {isGeneratingReport
                        ? "Generating report..."
                        : "Recording in progress"}
                    </span>
                  </div>

                  <button
                    onClick={handleStop}
                    disabled={!isRecording || isGeneratingReport}
                    className="flex items-center gap-2 px-4 py-2 bg-white dark:bg-zinc-800 hover:bg-zinc-50 dark:hover:bg-zinc-700 disabled:opacity-40 border border-zinc-200 dark:border-zinc-700 text-zinc-700 dark:text-zinc-300 text-sm font-medium rounded-lg transition-colors cursor-pointer disabled:cursor-not-allowed"
                  >
                    {isGeneratingReport ? (
                      <ArrowPathIcon className="w-4 h-4 animate-spin" />
                    ) : (
                      <StopIcon className="w-4 h-4" />
                    )}
                    {isGeneratingReport ? "Generating..." : "Stop & Save"}
                  </button>
                </div>
              </div>
            ) : !showReport ? (
              <div className="flex flex-col items-center justify-center flex-1 gap-10 py-16 select-none">
                <div className="flex flex-col items-center gap-3">
                  <MicrophoneIcon className="w-8 h-8 text-zinc-300 dark:text-zinc-700" />
                  <p className="text-sm text-zinc-400 dark:text-zinc-600 text-center leading-relaxed">
                    Click{" "}
                    <span className="font-medium text-zinc-500 dark:text-zinc-400">
                      New Session
                    </span>{" "}
                    in the sidebar to start recording.
                  </p>
                  <p className="text-xs text-zinc-300 dark:text-zinc-700 text-center leading-relaxed max-w-xs">
                    Using speakers instead of earphones? Your mic may also
                    pick up the other person's voice. Turn on macOS's{" "}
                    <span className="font-medium">Voice Isolation</span> mic
                    mode (Control Center → Mic Mode) to filter it out.
                  </p>
                </div>

                {stats && stats.total_sessions > 0 ? (
                  <div className="grid grid-cols-3 gap-3 w-full max-w-sm">
                    {[
                      {
                        label: "Sessions",
                        value: stats.total_sessions,
                        color: "text-zinc-800 dark:text-zinc-100",
                      },
                      {
                        label: "Utterances",
                        value: stats.total_utterances,
                        color: "text-zinc-800 dark:text-zinc-100",
                      },
                      {
                        label: "Corrections",
                        value: stats.total_corrections,
                        color: "text-red-600 dark:text-red-400",
                      },
                    ].map(({ label, value, color }) => (
                      <div
                        key={label}
                        className="min-w-0 border border-zinc-100 dark:border-zinc-800 rounded-lg p-3 text-center"
                      >
                        <div
                          className={`text-xl font-semibold tabular-nums ${color}`}
                        >
                          {value}
                        </div>
                        <div className="text-[11px] leading-tight text-zinc-400 dark:text-zinc-500 mt-0.5">
                          {label}
                        </div>
                      </div>
                    ))}
                  </div>
                ) : (
                  <div className="grid grid-cols-3 gap-3 w-full max-w-sm">
                    {["Sessions", "Utterances", "Corrections"].map(
                      (label) => (
                        <div
                          key={label}
                          className="min-w-0 border border-dashed border-zinc-200 dark:border-zinc-800 rounded-lg p-3 text-center"
                        >
                          <div className="text-xl font-semibold tabular-nums text-zinc-300 dark:text-zinc-700">
                            —
                          </div>
                          <div className="text-[11px] leading-tight text-zinc-300 dark:text-zinc-700 mt-0.5">
                            {label}
                          </div>
                        </div>
                      ),
                    )}
                  </div>
                )}
              </div>
            ) : null}

            {/* 리포트 상세 — 통계 + Corrections/Vocabulary 탭 (리포트 직후 또는 과거 세션 선택 시 표시) */}
            {showReport && (
              <div className="mt-5">
                <SessionReport
                  utterances={utterances}
                  reportPath={reportPath}
                  isVisible={true}
                  markdownContent={selectedSessionMarkdown}
                />
              </div>
            )}
          </main>

          {/* 오른쪽 패널 */}
          <aside className="w-80 shrink-0 border-l border-zinc-200 dark:border-zinc-800 bg-white dark:bg-zinc-900 flex flex-col overflow-hidden">
            <div className="flex items-center justify-between px-5 py-3.5 border-b border-zinc-100 dark:border-zinc-800 shrink-0">
              <h3 className="text-xs font-medium text-zinc-500 dark:text-zinc-400 uppercase tracking-wider">
                {rightPanelTitle}
              </h3>
              {canViewTranscript && (
                <button
                  onClick={handleToggleTranscript}
                  className="flex items-center gap-1 text-xs font-medium text-zinc-400 dark:text-zinc-500 hover:text-zinc-700 dark:hover:text-zinc-200 transition-colors cursor-pointer"
                >
                  <DocumentTextIcon className="w-3.5 h-3.5" />
                  {showTranscript
                    ? isRecording
                      ? "Hide"
                      : "Report"
                    : "Transcript"}
                </button>
              )}
            </div>
            <div className="flex-1 min-h-0 overflow-y-auto p-5">
              {showTranscript ? (
                <TranscriptView
                  text={
                    isRecording
                      ? liveTranscript
                          .map((u) => `[${u.timestamp}] ${u.text}`)
                          .join("\n")
                      : loadedTranscript
                  }
                />
              ) : isGeneratingReport ? (
                <div className="flex flex-col items-center justify-center h-full gap-3 select-none">
                  <ArrowPathIcon className="w-5 h-5 text-zinc-400 dark:text-zinc-600 animate-spin" />
                  <p className="text-xs text-zinc-400 dark:text-zinc-500 text-center leading-relaxed">
                    Generating report from your transcript...
                  </p>
                </div>
              ) : isRecording ? (
                <div className="flex flex-col items-center justify-center h-full gap-3 select-none">
                  <MicrophoneIcon className="w-7 h-7 text-zinc-300 dark:text-zinc-600" />
                  <p className="text-xs text-zinc-400 dark:text-zinc-500 text-center leading-relaxed">
                    Recording — {liveTranscript.length} utterance
                    {liveTranscript.length === 1 ? "" : "s"} captured.
                    <br />
                    Feedback is generated when you stop.
                  </p>
                </div>
              ) : showReport ? (
                <SessionLog utterances={utterances} />
              ) : (
                <div className="flex flex-col items-center justify-center h-full gap-2 select-none">
                  <p className="text-xs text-zinc-400 dark:text-zinc-600 text-center leading-relaxed">
                    Select a session from the sidebar
                    <br />
                    or start a new recording
                  </p>
                </div>
              )}
            </div>
          </aside>
        </div>
      </div>
    </div>
  );
}

export default App;
