import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import {
  ArrowPathIcon,
  DocumentTextIcon,
  DocumentArrowUpIcon,
  ChevronLeftIcon,
  ChevronRightIcon,
  PencilIcon,
  PlusIcon,
  TrashIcon,
  CheckIcon,
  XMarkIcon,
} from "@heroicons/react/20/solid";
import appIcon from "../assets/app-icon.png";
import { SessionSource } from "../types";

export interface SessionInfo {
  filename: string;
  title: string;
  path: string;
  source: SessionSource;
}

interface SessionListProps {
  selectedSession: SessionInfo | null;
  onSelectSession: (session: SessionInfo) => void;
  onRenameSession: (session: SessionInfo, newTitle: string) => void;
  onDeleteSession: (session: SessionInfo) => void;
  onNewSession: () => void;
  onUploadClick: () => void;
  newSessionDisabled?: boolean;
  refreshKey?: number;
}

export function SessionList({
  selectedSession,
  onSelectSession,
  onRenameSession,
  onDeleteSession,
  onNewSession,
  onUploadClick,
  newSessionDisabled,
  refreshKey,
}: SessionListProps) {
  const [sessions, setSessions] = useState<SessionInfo[]>([]);
  const [loading, setLoading] = useState(true);
  const [collapsed, setCollapsed] = useState(false);
  const [editingPath, setEditingPath] = useState<string | null>(null);
  const [editValue, setEditValue] = useState("");
  const [confirmingPath, setConfirmingPath] = useState<string | null>(null);

  const loadSessions = async () => {
    try {
      setLoading(true);
      const list = await invoke<SessionInfo[]>("list_sessions");
      setSessions(list);
    } catch {
      setSessions([]);
    } finally {
      setLoading(false);
    }
  };

  useEffect(() => {
    loadSessions();
  }, [refreshKey]);

  const startRename = (session: SessionInfo) => {
    setEditingPath(session.path);
    setEditValue(session.title);
  };

  const commitRename = (session: SessionInfo) => {
    setEditingPath(null);
    if (editValue.trim() && editValue.trim() !== session.title) {
      onRenameSession(session, editValue.trim());
    }
  };

  // Tauri 웹뷰에서는 window.confirm이 제대로 동작하지 않을 수 있어
  // 인라인 2단계 확인 UI로 대체 (휴지통 클릭 → 확인/취소 버튼 노출)
  const handleTrashClick = (session: SessionInfo) => {
    setConfirmingPath(session.path);
  };

  const confirmDelete = (session: SessionInfo) => {
    setConfirmingPath(null);
    onDeleteSession(session);
  };

  return (
    <nav
      className={`flex flex-col shrink-0 bg-zinc-50 dark:bg-zinc-950 text-zinc-900 dark:text-zinc-100 border-r border-zinc-200 dark:border-transparent overflow-hidden transition-[width] duration-200 ${
        collapsed ? "w-16" : "w-56"
      }`}
    >
      {/* 앱 로고 영역 */}
      <div
        className={`flex items-center justify-between h-16 shrink-0 border-b border-zinc-200 dark:border-zinc-800 ${
          collapsed ? "px-2" : "px-4"
        }`}
      >
        <img
          src={appIcon}
          alt="Scoldler"
          className="w-6 h-6 rounded shrink-0"
        />
        <button
          onClick={() => setCollapsed((c) => !c)}
          className="p-1 rounded text-zinc-400 dark:text-zinc-600 hover:text-zinc-600 dark:hover:text-zinc-300 hover:bg-zinc-200 dark:hover:bg-zinc-800 transition-colors shrink-0"
        >
          {collapsed ? (
            <ChevronRightIcon className="w-3.5 h-3.5" />
          ) : (
            <ChevronLeftIcon className="w-3.5 h-3.5" />
          )}
        </button>
      </div>

      {/* 새 세션 버튼 - 녹음 시작 또는 파일 업로드 */}
      <div className={`pt-3 shrink-0 flex gap-1.5 ${collapsed ? "px-1.5 flex-col" : "px-2"}`}>
        <button
          onClick={onNewSession}
          disabled={newSessionDisabled}
          title="New Session"
          className={`flex items-center gap-2 flex-1 rounded-md text-xs font-medium bg-zinc-900/5 dark:bg-white/10 text-zinc-900 dark:text-white hover:bg-zinc-900/10 dark:hover:bg-white/15 disabled:opacity-40 disabled:cursor-not-allowed transition-colors cursor-pointer ${
            collapsed ? "justify-center p-2" : "px-2.5 py-2"
          }`}
        >
          <PlusIcon className="w-3.5 h-3.5 shrink-0" />
          {!collapsed && "New Session"}
        </button>
        <button
          onClick={onUploadClick}
          disabled={newSessionDisabled}
          title="Upload Recording"
          className={`flex items-center justify-center rounded-md text-xs font-medium bg-zinc-900/5 dark:bg-white/10 text-zinc-900 dark:text-white hover:bg-zinc-900/10 dark:hover:bg-white/15 disabled:opacity-40 disabled:cursor-not-allowed transition-colors cursor-pointer ${
            collapsed ? "p-2" : "px-2.5 py-2"
          }`}
        >
          <DocumentArrowUpIcon className="w-3.5 h-3.5 shrink-0" />
        </button>
      </div>

      {/* Sessions 섹션 */}
      <div
        className={`flex-1 overflow-y-auto py-3 ${collapsed ? "px-1.5" : "px-2"}`}
      >
        {!collapsed && (
          <div className="flex items-center justify-between px-2 mb-2">
            <span className="text-xs font-medium text-zinc-400 dark:text-zinc-500 uppercase tracking-wider">
              Sessions
            </span>
            <button
              onClick={loadSessions}
              className="p-1 rounded text-zinc-400 dark:text-zinc-600 hover:text-zinc-600 dark:hover:text-zinc-300 hover:bg-zinc-200 dark:hover:bg-zinc-800 transition-colors"
            >
              <ArrowPathIcon className="w-3 h-3" />
            </button>
          </div>
        )}

        {collapsed ? null : loading ? (
          <div className="px-2 py-4 text-xs text-zinc-400 dark:text-zinc-600">
            Loading...
          </div>
        ) : sessions.length === 0 ? (
          <div className="px-2 py-6 flex flex-col items-center gap-2 text-center">
            <DocumentTextIcon className="w-6 h-6 text-zinc-300 dark:text-zinc-700" />
            <p className="text-xs text-zinc-400 dark:text-zinc-600 leading-relaxed">
              No sessions yet.
              <br />
              Start recording to create one.
            </p>
          </div>
        ) : (
          <ul className="space-y-0.5">
            {sessions.map((session) => {
              const isSelected = selectedSession?.filename === session.filename;
              const isEditing = editingPath === session.path;

              if (isEditing) {
                return (
                  <li key={session.filename} className="px-0.5">
                    <input
                      autoFocus
                      value={editValue}
                      onChange={(e) => setEditValue(e.target.value)}
                      onBlur={() => commitRename(session)}
                      onKeyDown={(e) => {
                        if (e.key === "Enter") commitRename(session);
                        if (e.key === "Escape") setEditingPath(null);
                      }}
                      className="w-full px-2 py-1.5 text-xs rounded-md border border-zinc-300 dark:border-zinc-700 bg-white dark:bg-zinc-900 text-zinc-900 dark:text-zinc-100 outline-none focus:ring-1 focus:ring-zinc-400 dark:focus:ring-zinc-500"
                    />
                  </li>
                );
              }

              const isConfirming = confirmingPath === session.path;

              return (
                <li
                  key={session.filename}
                  className="group relative"
                  onMouseLeave={() => isConfirming && setConfirmingPath(null)}
                >
                  <button
                    onClick={() => onSelectSession(session)}
                    className={`w-full text-left rounded-md text-xs transition-colors px-2 py-2 pr-12 ${
                      isSelected
                        ? "bg-zinc-900/5 dark:bg-white/10 text-zinc-900 dark:text-white"
                        : "text-zinc-500 dark:text-zinc-400 hover:bg-zinc-900/5 dark:hover:bg-white/5 hover:text-zinc-700 dark:hover:text-zinc-200"
                    }`}
                  >
                    <div className="font-medium truncate flex items-center gap-1.5">
                      {session.title}
                      {session.source.kind === "uploaded" && (
                        <span
                          title={`Uploaded: ${session.source.original_filename}`}
                          className="shrink-0 text-[9px] font-semibold uppercase tracking-wide px-1 py-0.5 rounded bg-zinc-200 dark:bg-zinc-700 text-zinc-500 dark:text-zinc-300"
                        >
                          File
                        </span>
                      )}
                    </div>
                    <div className="text-zinc-400 dark:text-zinc-600 font-mono truncate mt-0.5 text-[10px]">
                      {session.filename.replace(".md", "")}
                    </div>
                  </button>

                  <div
                    className={`absolute right-1.5 top-1.5 items-center gap-0.5 ${
                      isConfirming ? "flex" : "hidden group-hover:flex"
                    }`}
                  >
                    {isConfirming ? (
                      <>
                        <button
                          onClick={() => confirmDelete(session)}
                          title="Confirm delete"
                          className="p-1 rounded bg-red-600 text-white hover:bg-red-700 transition-colors"
                        >
                          <CheckIcon className="w-3 h-3" />
                        </button>
                        <button
                          onClick={() => setConfirmingPath(null)}
                          title="Cancel"
                          className="p-1 rounded text-zinc-400 dark:text-zinc-600 hover:text-zinc-700 dark:hover:text-zinc-200 hover:bg-zinc-200 dark:hover:bg-zinc-700 transition-colors"
                        >
                          <XMarkIcon className="w-3 h-3" />
                        </button>
                      </>
                    ) : (
                      <>
                        <button
                          onClick={() => startRename(session)}
                          title="Rename"
                          className="p-1 rounded text-zinc-400 dark:text-zinc-600 hover:text-zinc-700 dark:hover:text-zinc-200 hover:bg-zinc-200 dark:hover:bg-zinc-700 transition-colors"
                        >
                          <PencilIcon className="w-3 h-3" />
                        </button>
                        <button
                          onClick={() => handleTrashClick(session)}
                          title="Delete"
                          className="p-1 rounded text-zinc-400 dark:text-zinc-600 hover:text-red-600 dark:hover:text-red-400 hover:bg-red-50 dark:hover:bg-red-950/50 transition-colors"
                        >
                          <TrashIcon className="w-3 h-3" />
                        </button>
                      </>
                    )}
                  </div>
                </li>
              );
            })}
          </ul>
        )}
      </div>
    </nav>
  );
}
