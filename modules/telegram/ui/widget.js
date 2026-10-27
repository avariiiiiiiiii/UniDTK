import { invoke, convertFileSrc, listen, openPath } from '@unidtk/bridge';
import { saveConfig, showDebugMessage, logFrontend, repairMojibake, editMode, widgetsCanvas, config, defaultWidgetPosition, cachedTelegramCard, escapeHtml } from '../../unidtk-core.js';
import { surfaceManager, syncSurfaceWash } from '../../surface-manager.js';
import { calculateDuration, getDuration, prefersReducedMotion, EASINGS, motionTimeline } from '../../motion.js';
import { getFileIconName, getFileTypeName } from '../../file-icons.js';

const telegramRuntime = {
  status: null,
  statusLoaded: false,
  connectionState: "initializing",
  config: null,
  configLoaded: false,
  debug: null,
  debugLoaded: false,
  chats: null,
  archivedChats: null,
  chatsLoaded: false,
  archivedChatsLoaded: false,
  loadingArchivedChats: false,
  chatListView: "main",
  chatsError: "",
  messages: new Map(),
  messageCacheOrder: [],
  messagesLoaded: new Set(),
  messagesError: new Map(),
  loadingChats: false,
  loadingMessages: new Set(),
  statusPollTimer: null,
  chatPollTimer: null,
  chatPollAttempts: 0,
  syncTimer: null,
  loadingEarlier: new Set(),
  historyExhausted: new Set(),
  earlierCooldownUntil: new Map(),
  pendingScrollAnchor: null,
  authSubmitting: false,
  drafts: new Map(),
  chatFilter: "",
  chatVisibleLimit: 24,
  lastMarkedRead: new Map(),
  /** True while a text send invoke is in flight — blocks double Enter/click. */
  sending: false,
  /** File path from explorer "Send to Telegram" — pending in the composer. */
  pendingExternalFile: null,
  // Lazily created caches (avatar/media). Reset on logout/re-auth.
  mediaDownloads: null,
  mediaFailed: null,
  avatarRetry: null,
  avatarRenderFailures: null,
  /** Cache of forum topics per chatId: Map<chatId, Array<{info, lastMessage}>> */
  forumTopics: new Map(),
  forumTopicsLoading: new Set(),
  forumFilterByChat: new Map(),
  /** Selected forum thread per forum chat; thread IDs are never global. */
  forumTopicByChat: new Map(),
  /** User-paused GIF animations, keyed by chat/message so renders preserve choice. */
  pausedAnimations: new Set(),
  accountGeneration: 0,
  pinnedMessages: new Map(),
  pinnedMessageSnippet: new Map(),
  /** pinKey → timestamp until which failed pin lookups are not retried. */
  pinnedCooldown: new Map(),
  /** Cache of custom emoji sticker paths: Map<customEmojiId, localFilePath> */
  customEmojiPaths: new Map(),
  /** Track custom emoji paths currently being queried to prevent duplicate requests */
  pendingCustomEmojiFetches: new Set(),
  /** Detached history state when viewing history around pinned message or quote */
  detachedState: null,
  /** Monotonic jump token to cancel stale jumps */
  jumpToken: 0,
};

function getTelegramCard() {
  if (cachedTelegramCard && cachedTelegramCard.isConnected) {
    return cachedTelegramCard;
  }
  const found = document.querySelector(".widget-card.telegram-widget, .telegram-widget, [data-widget='telegram'], [data-id='telegram']");
  return (found && found.isConnected) ? found : found || null;
}

function requestTelegramWidgetRender(card, widgetId) {
  if (!card?.isConnected) return;
  if (card._tgSurfaceTransitioning || card._tgTransitioning) {
    card._tgPendingUpdate = true;
    return;
  }
  renderTelegramWidget(card, widgetId);
}
window.__telegramRuntime = telegramRuntime;

/** Clear all per-chat / per-account client state (logout or switch account). */
function clearTelegramAccountState() {
  document.querySelectorAll('.telegram-widget').forEach(card => {
    cancelActiveTelegramMorph(card);
    card._tgDockedScroll = null;
    card._tgActivatePrepared = null;
  });
  telegramRuntime.chats = null;
  telegramRuntime.archivedChats = null;
  telegramRuntime.chatsLoaded = false;
  telegramRuntime.archivedChatsLoaded = false;
  telegramRuntime.loadingArchivedChats = false;
  telegramRuntime.chatListView = "main";
  telegramRuntime.chatsError = "";
  telegramRuntime.messages = new Map();
  telegramRuntime.messageCacheOrder = [];
  telegramRuntime.messagesLoaded = new Set();
  telegramRuntime.messagesError = new Map();
  telegramRuntime.loadingChats = false;
  telegramRuntime.loadingMessages = new Set();
  telegramRuntime.chatPollAttempts = 0;
  clearTimeout(telegramRuntime.chatPollTimer);
  telegramRuntime.chatPollTimer = null;
  clearTimeout(telegramRuntime.archiveRefreshTimer);
  telegramRuntime.archiveRefreshTimer = null;
  telegramRuntime.loadingEarlier = new Set();
  telegramRuntime.historyExhausted = new Set();
  telegramRuntime.earlierCooldownUntil = new Map();
  telegramRuntime.pendingScrollAnchor = null;
  telegramRuntime.pendingExternalFile = null;
  telegramRuntime.sending = false;
  telegramRuntime.drafts = new Map();
  if (telegramRuntime.replyMessage) telegramRuntime.replyMessage.clear();
  if (telegramRuntime.editMessage) telegramRuntime.editMessage.clear();
  if (telegramRuntime.pinnedMessages) telegramRuntime.pinnedMessages.clear();
  if (telegramRuntime.pinnedMessageSnippet) telegramRuntime.pinnedMessageSnippet.clear();
  if (telegramRuntime.pinnedCooldown) telegramRuntime.pinnedCooldown.clear();
  telegramRuntime.chatFilter = "";
  telegramRuntime.chatVisibleLimit = TELEGRAM_CHAT_PAGE_SIZE;
  telegramRuntime.lastMarkedRead = new Map();
  telegramRuntime.mediaDownloads = null;
  telegramRuntime.mediaFailed = null;
  telegramRuntime.avatarRetry = null;
  telegramRuntime.avatarRenderFailures = null;
  telegramRuntime.accountGeneration += 1;
  telegramRuntime.forumTopics.clear();
  telegramRuntime.forumTopicsLoading.clear();
  telegramRuntime.forumFilterByChat.clear();
  telegramRuntime.forumTopicByChat.clear();
  telegramRuntime.pausedAnimations.clear();
  if (telegramRuntime.customEmojiPaths) telegramRuntime.customEmojiPaths.clear();
  if (telegramRuntime.pendingCustomEmojiFetches) telegramRuntime.pendingCustomEmojiFetches.clear();
  telegramRuntime.detachedState = null;
  telegramRuntime.jumpToken = 0;
  tgMention.open = false;
  tgMention.items = [];
  tgMention.entities = [];
}

const TELEGRAM_DEFAULT_TDJSON_PATH = "";
const TELEGRAM_CHAT_PAGE_SIZE = 24;
const TELEGRAM_RAIL_CHAT_LIMIT = 14;
const TELEGRAM_MAX_CACHED_CHATS = 6;
const TELEGRAM_MAX_CACHED_MESSAGES = 300;

// TDLib persists the full history on disk. This Map is only a renderer working
// set, so keeping every opened chat and an unlimited message history here just
// duplicates the database in WebView memory.
function telegramForumThreadId(chatId) {
  if (chatId == null || chatId === "") return null;
  const threadId = telegramRuntime.forumTopicByChat.get(String(chatId));
  return threadId && String(threadId) !== "0" ? String(threadId) : null;
}

function setTelegramForumThread(chatId, threadId) {
  if (chatId == null || chatId === "") return;
  telegramRuntime.chatSwitchEpoch = (telegramRuntime.chatSwitchEpoch || 0) + 1;
  const key = String(chatId);
  const prevThread = telegramRuntime.forumTopicByChat.get(key) || null;
  const nextThread = (threadId == null || threadId === "" || String(threadId) === "0") ? null : String(threadId);
  if (prevThread !== nextThread && telegramRuntime.detachedState) {
    telegramRuntime.detachedState = null;
  }
  if (!nextThread) {
    telegramRuntime.forumTopicByChat.delete(key);
  } else {
    telegramRuntime.forumTopicByChat.set(key, nextThread);
  }
}

function telegramThreadCacheKey(chatId, threadId = null) {
  return threadId && String(threadId) !== "0"
    ? `${chatId}::${String(threadId)}`
    : String(chatId || "");
}

function cacheTelegramMessages(chatId, messages, threadId = telegramForumThreadId(chatId)) {
  if (!chatId) return [];
  const list = Array.isArray(messages) ? messages : [];
  const capped = list.length > TELEGRAM_MAX_CACHED_MESSAGES
    ? list.slice(-TELEGRAM_MAX_CACHED_MESSAGES)
    : list;
  // Forum topic messages are cached under `chatId::<threadId>` so they never
  // mix with the main chat window (matches the backend cache keys).
  const key = telegramThreadCacheKey(chatId, threadId);
  telegramRuntime.messages.set(key, capped);

  const order = telegramRuntime.messageCacheOrder;
  const previous = order.indexOf(key);
  if (previous !== -1) order.splice(previous, 1);
  order.push(key);
  while (order.length > TELEGRAM_MAX_CACHED_CHATS) {
    const evicted = order.shift();
    if (evicted === key) continue;
    telegramRuntime.messages.delete(evicted);
    telegramRuntime.messagesLoaded.delete(evicted);
    telegramRuntime.messagesError.delete(evicted);
    telegramRuntime.historyExhausted.delete(evicted);
    telegramRuntime.earlierCooldownUntil.delete(evicted);
  }
  return capped;
}

/** Current message cache key for a chat (respects the active forum topic). */
function telegramMessageCacheKey(chatId, threadId = telegramForumThreadId(chatId)) {
  return telegramThreadCacheKey(chatId, threadId);
}

function telegramMessageThreadId(message, chatId) {
  const cid = String(chatId ?? message?.chatId ?? message?.chat_id ?? "");
  if (cid) {
    const chat = (telegramRuntime.chats || []).find((c) => String(c.id) === cid)
      || (telegramRuntime.archivedChats || []).find((c) => String(c.id) === cid);
    if (chat && !chat.isForum) {
      return null;
    }
  }
  const value = message?.messageThreadId ?? message?.message_thread_id;
  const id = value == null ? null : String(value);
  return id && id !== "0" ? id : null;
}

function telegramCachedMessageKey(chatId, messageId) {
  const base = String(chatId || "");
  const keys = [base, ...telegramRuntime.messages.keys()].filter((key, index, all) =>
    (key === base || String(key).startsWith(`${base}::`)) && all.indexOf(key) === index,
  );
  for (const key of keys) {
    if ((telegramRuntime.messages.get(key) || []).some((message) => String(message.id) === String(messageId))) return key;
  }
  return telegramMessageCacheKey(chatId);
}

function shouldShowTelegramAuth(status) {
    if (!status || status.authorized) return false;
  return status.mode === "tdlib" ||
    [
      "missing_config",
      "saved_config",
      "error",
      "WaitPhoneNumber",
      "WaitCode",
      "WaitPassword",
      "WaitEmailAddress",
      "WaitEmailCode",
      "WaitRegistration",
      "WaitOtherDeviceConfirmation",
      "WaitEncryptionKey",
      "starting",
    ].includes(status.authState);
}

/** Snapshot live auth/settings inputs before a full widget rebuild wipes them. */
function captureTelegramAuthFields(body) {
  if (!body?.querySelector?.(".tg-auth")) return null;
  const active = document.activeElement;
  const fields = {};
  body.querySelectorAll(".tg-auth .tg-auth-field").forEach((el) => {
    const key = el.dataset.tgConfig || el.dataset.tgAuth;
    if (!key) return;
    fields[key] = {
      value: el.value ?? "",
      hadFocus: active === el,
      selectionStart: el.selectionStart ?? 0,
      selectionEnd: el.selectionEnd ?? 0,
    };
  });
  return fields;
}

/** Restore captured auth/settings values + focus/cursor after re-render. */
function restoreTelegramAuthFields(body, captured) {
  if (!captured || !body) return;
  body.querySelectorAll(".tg-auth .tg-auth-field").forEach((el) => {
    const key = el.dataset.tgConfig || el.dataset.tgAuth;
    const snap = key ? captured[key] : null;
    if (!snap) return;
    el.value = snap.value;
    if (snap.hadFocus) {
      el.focus();
      try {
        if (
          typeof el.setSelectionRange === "function" &&
          el.tagName !== "SELECT" &&
          el.type !== "number"
        ) {
          el.setSelectionRange(snap.selectionStart, snap.selectionEnd);
        }
      } catch (_) { /* ignore selection restore failures */ }
    }
  });
}

function telegramVisibleChats() {
  const main = telegramRuntime.chats || [];
  const archived = telegramRuntime.archivedChats || [];
  const archivedIds = new Set(archived.map((chat) => String(chat.id)));
  const source = telegramRuntime.chatListView === "archive"
    ? archived
    : main.filter((chat) => !archivedIds.has(String(chat.id)));
  // Show the full loaded list: favorites first, then the rest. Do not slice —
  // backend already returns the complete chat list once TDLib has been drained.
  const favorites = source.filter((chat) => chat.favorite);
  const recent = source.filter((chat) => !chat.favorite);
  return [...favorites, ...recent];
}

function telegramRenderedChats(chats) {
  const filter = (telegramRuntime.chatFilter || "").trim().toLowerCase();
  const filtered = filter
    ? chats.filter((chat) => (chat.title || "").toLowerCase().includes(filter))
    : chats;
  const limit = Math.max(TELEGRAM_CHAT_PAGE_SIZE, telegramRuntime.chatVisibleLimit || 0);
  return filtered.slice(0, limit);
}

// Fields that affect rail/list rendering. Avoid JSON.stringify of whole chat
// objects (slow on large lists; false positives from key order / non-UI fields).
function telegramChatsRenderChanged(prev, next) {
  if (prev === next) return false;
  if (!Array.isArray(prev) || !Array.isArray(next)) return prev !== next;
  if (prev.length !== next.length) return true;
  for (let i = 0; i < prev.length; i++) {
    const a = prev[i];
    const b = next[i];
    if (!a || !b) return true;
    if (
      a.id !== b.id ||
      a.order !== b.order ||
      a.unread !== b.unread ||
      a.favorite !== b.favorite ||
      a.muted !== b.muted ||
      a.preview !== b.preview ||
      a.time !== b.time ||
      a.title !== b.title ||
      a.kind !== b.kind ||
      a.isForum !== b.isForum ||
      a.photoPath !== b.photoPath
    ) {
      return true;
    }
  }
  return false;
}

async function refreshTelegramStatus(card, widgetId) {
  if (telegramRuntime.statusLoaded) return;
  telegramRuntime.statusLoaded = true;
  try {
    const status = await invoke("telegram_status");
    if (status) {
      const previousStatus = telegramRuntime.status;
      const becameAuthorized = status.authorized && !previousStatus?.authorized;
      const becameUnauthorized = !status.authorized && !!previousStatus?.authorized;
      const statusChanged = !previousStatus || [
        "mode",
        "authorized",
        "authState",
        "authHint",
        "activeAccountId",
      ].some((key) => previousStatus[key] !== status[key])
        || JSON.stringify((previousStatus.accounts || []).map((a) => a.id)) !== JSON.stringify((status.accounts || []).map((a) => a.id));
      telegramRuntime.status = status;
      if (status.connectionState) {
        const prevConnState = telegramRuntime.connectionState;
        telegramRuntime.connectionState = status.connectionState;
        const targetCard = card || getTelegramCard();
        if (targetCard) updateTelegramConnectionStrip(targetCard, status.connectionState);
        if (prevConnState !== "ready" && status.connectionState === "ready") {
          const now = Date.now();
          if (!telegramRuntime.lastReadyCatchupAt || now - telegramRuntime.lastReadyCatchupAt > 10000) {
            telegramRuntime.lastReadyCatchupAt = now;
            const activeId = config.positions?.[widgetId]?.activeChatId || "";
            if (activeId && telegramRuntime.messagesLoaded.has(telegramMessageCacheKey(activeId))) {
              if (targetCard) refreshTelegramMessages(targetCard, widgetId, activeId, true);
            }
            if (targetCard) refreshTelegramChats(targetCard, widgetId, true);
          }
        }
      }
      // Logout (authorized → not) or login as a possibly different account both
      // must drop every per-chat/per-account client cache.
      if (becameUnauthorized || becameAuthorized) {
        clearTelegramAccountState();
      }
      // Polling must not rebuild the DOM while the user is typing their phone,
      // login code, or 2FA password: rebuilding would destroy input focus.
      // Snapshots help, but WorkerW keyboard focus is fragile — skip rebuild
      // unless auth actually advanced (state change that needs a new form).
      if (statusChanged) {
        const ae = document.activeElement;
        const typingInAuth =
          !!ae
          && card.contains(ae)
          && (ae.matches?.(".tg-auth-field, .tg-input, .tg-chat-search")
            || !!ae.closest?.(".tg-auth"));
        const authFormChanged = previousStatus?.authState !== status.authState
          || becameAuthorized
          || becameUnauthorized;
        if (!typingInAuth || authFormChanged) {
          requestTelegramWidgetRender(card, widgetId);
        }
      }
      if (!status.authorized && card.isConnected) {
        clearTimeout(telegramRuntime.statusPollTimer);
        telegramRuntime.statusPollTimer = setTimeout(() => {
          telegramRuntime.statusLoaded = false;
          refreshTelegramStatus(card, widgetId);
        }, 750);
      }
    }
  } catch {
    telegramRuntime.statusLoaded = false;
  }
}

async function refreshTelegramConfig(card, widgetId) {
  if (telegramRuntime.configLoaded) return;
  telegramRuntime.configLoaded = true;
  try {
    telegramRuntime.config = await invoke("telegram_get_config");
    requestTelegramWidgetRender(card, widgetId);
  } catch {
    telegramRuntime.configLoaded = false;
  }
}

async function refreshTelegramDebug(card, widgetId) {
  if (telegramRuntime.debugLoaded) return;
  telegramRuntime.debugLoaded = true;
  try {
    telegramRuntime.debug = await invoke("telegram_debug");
    requestTelegramWidgetRender(card, widgetId);
  } catch {
    telegramRuntime.debugLoaded = false;
  }
}

async function refreshTelegramChats(card, widgetId, force = false) {
  if ((!force && telegramRuntime.chatsLoaded) || telegramRuntime.loadingChats) return;
  if (force && telegramRuntime.chatPollTimer) {
    clearTimeout(telegramRuntime.chatPollTimer);
    telegramRuntime.chatPollTimer = null;
  }
  telegramRuntime.loadingChats = true;
  telegramRuntime.chatsError = "";
  const generation = telegramRuntime.accountGeneration;
  try {
    const fetched = await invoke("telegram_chats");
    if (generation !== telegramRuntime.accountGeneration) return;
    if (Array.isArray(fetched)) {
      // `photoPath` is a frontend-only field filled in after a successful
      // avatar download (see handleTelegramAvatarDownload) — the backend chat
      // objects never carry it. Re-attach it by id so a routine chat refresh
      // doesn't discard already-downloaded avatars and force a re-download.
      const previousById = new Map((telegramRuntime.chats || []).map((chat) => [chat.id, chat]));
      const chats = fetched.map((chat) => {
        const prev = previousById.get(chat.id);
        // Prefer a path freshly reported by backend (TDLib snapshot or file-update).
        // Fall back to a previously downloaded frontend-cached path for the same file.
        if (chat.photoPath) return chat;
        if (prev?.photoPath && chat.photoFileId === prev.photoFileId) {
          return { ...chat, photoPath: prev.photoPath };
        }
        return chat;
      });
      const previousCount = (telegramRuntime.chats || []).length;
      const changed = telegramChatsRenderChanged(telegramRuntime.chats || [], chats);
      const wasLoaded = telegramRuntime.chatsLoaded;
      telegramRuntime.chats = chats;
      telegramRuntime.chatsLoaded = true;
      if (!wasLoaded || changed) updateTelegramWidget(card, widgetId);
      // `loadChats` arrives as asynchronous updateNewChat events and a first
      // read can be a partial non-empty snapshot. Keep refreshing until the
      // list is stable across two reads; otherwise startup could freeze one
      // early chat in the UI even though TDLib has since loaded the full list.
      const shouldDrain = card.isConnected
        && telegramRuntime.chatPollAttempts < 15
        && ((chats.length > 0 && (!wasLoaded || chats.length !== previousCount))
          || (!chats.length && telegramRuntime.status?.authorized));
      if (shouldDrain) {
        telegramRuntime.chatPollAttempts += 1;
        clearTimeout(telegramRuntime.chatPollTimer);
        telegramRuntime.chatPollTimer = setTimeout(() => {
          telegramRuntime.chatsLoaded = false;
          refreshTelegramChats(card, widgetId);
        }, 800);
      } else {
        telegramRuntime.chatPollAttempts = 0;
      }
    }
  } catch (error) {
    if (generation === telegramRuntime.accountGeneration) {
      telegramRuntime.chatsError = String(error?.message || error);
      // Surface the failure: without a re-render the DOM stays on the last
      // paint (often the loading skeleton) while chatsError is already set —
      // same pattern as the message-thread error surface below.
      if (card?.isConnected) requestTelegramWidgetRender(card, widgetId);
    }
  } finally {
    if (generation === telegramRuntime.accountGeneration) telegramRuntime.loadingChats = false;
  }
}

async function refreshTelegramArchiveChats(card, widgetId, force = false) {
  if ((!force && telegramRuntime.archivedChatsLoaded) || telegramRuntime.loadingArchivedChats) return;
  telegramRuntime.loadingArchivedChats = true;
  telegramRuntime.archivedChatsError = "";
  const generation = telegramRuntime.accountGeneration;
  try {
    const result = await invoke("telegram_chats_archive");
    if (generation !== telegramRuntime.accountGeneration) return;
    const fetched = Array.isArray(result) ? result : (Array.isArray(result?.chats) ? result.chats : []);
    const previousById = new Map((telegramRuntime.archivedChats || []).map((chat) => [String(chat.id), chat]));
    telegramRuntime.archivedChats = fetched.map((chat) => {
      const previous = previousById.get(String(chat.id));
      return !chat.photoPath && previous?.photoPath && chat.photoFileId === previous.photoFileId
        ? { ...chat, photoPath: previous.photoPath }
        : chat;
    });
    telegramRuntime.archivedChatsLoaded = true;
    if (telegramRuntime.chatListView === "archive") updateTelegramWidget(card, widgetId);
  } catch (error) {
    if (generation === telegramRuntime.accountGeneration) {
      telegramRuntime.archivedChatsError = String(error?.message || error);
    }
  } finally {
    if (generation === telegramRuntime.accountGeneration) telegramRuntime.loadingArchivedChats = false;
  }
}

async function setTelegramChatArchived(card, widgetId, chatId, archived) {
  if (!chatId) return;
  const generation = telegramRuntime.accountGeneration;
  try {
    await invoke("telegram_set_archived", { request: { chatId: String(chatId), archived: !!archived } });
    if (generation !== telegramRuntime.accountGeneration) return;
    await Promise.all([
      refreshTelegramChats(card, widgetId, true),
      refreshTelegramArchiveChats(card, widgetId, true),
    ]);
    if (generation !== telegramRuntime.accountGeneration) return;
    updateTelegramWidget(card, widgetId);
  } catch (error) {
    if (generation !== telegramRuntime.accountGeneration) return;
    logFrontend(`telegram archive action failed: ${String(error?.message || error)}`);
  }
}

// Merge a freshly fetched newest-messages window with what we already hold,
// keeping older history above it. Telegram message ids increase with time
// within a chat, so anything with an id below the fetched window is older.
// Optimistic just-sent rows (empty id) are dropped — the fetched window is the
// source of truth for the recent range and already contains them.
function tgMergeMessages(existing, fetched) {
  if (!Array.isArray(fetched) || !fetched.length) return existing;
  if (!Array.isArray(existing) || !existing.length) return fetched;
  const fetchedIds = new Set(fetched.map((m) => m.id).filter(Boolean));
  let minFetched = Infinity;
  for (const m of fetched) {
    const n = Number(m.id);
    if (Number.isFinite(n) && n > 0 && n < minFetched) minFetched = n;
  }
  if (!Number.isFinite(minFetched)) return fetched;
  const older = existing.filter((m) => {
    const n = Number(m.id);
    return m.id && Number.isFinite(n) && n > 0 && n < minFetched && !fetchedIds.has(m.id);
  });
  return older.length ? [...older, ...fetched] : fetched;
}

async function refreshTelegramMessages(card, widgetId, chatId, force = false) {
  if (!chatId) return;
  const threadId = telegramForumThreadId(chatId);
  const stateKey = telegramMessageCacheKey(chatId, threadId);
  if ((!force && telegramRuntime.messagesLoaded.has(stateKey)) || telegramRuntime.loadingMessages.has(stateKey)) return;
  const generation = telegramRuntime.accountGeneration;
  telegramRuntime.loadingMessages.add(stateKey);
  telegramRuntime.messagesError.delete(stateKey);
  try {
    const messages = await invoke("telegram_messages", { chatId, threadId: threadId ? Number(threadId) || null : null });
    if (generation !== telegramRuntime.accountGeneration) return;
    if (Array.isArray(messages)) {
      // A background refresh only fetches the newest window. Merging keeps any
      // older history the user already scrolled in, so a passive update never
      // wipes it out and yanks the view (see loadTelegramOlderMessages).
      const existing = telegramRuntime.messages.get(stateKey) || [];
      const merged = tgMergeMessages(existing, messages);
      cacheTelegramMessages(chatId, merged, threadId);
      telegramRuntime.messagesLoaded.add(stateKey);
      const stillSelected = telegramForumThreadId(chatId) === threadId;
      if (config.positions[widgetId]?.activeChatId === chatId && stillSelected) {
        // Always mark-read after history loads — not only when the DOM needs a
        // rebuild. Previously `!changed` skipped updateTelegramWidget and thus
        // never sent viewMessages for an already-cached window.
        if (card._tgTransitioning) {
          card._tgPendingUpdate = true;
        } else {
          updateTelegramWidget(card, widgetId);
        }
        if (merged.length < 25 && !telegramRuntime.historyExhausted?.has(stateKey)) {
          loadTelegramOlderMessages(card, widgetId, chatId);
        }
      }
    }
  } catch (error) {
    if (generation !== telegramRuntime.accountGeneration) return;
    telegramRuntime.messagesError.set(stateKey, String(error?.message || error));
    // Surface the failure: without a re-render the thread stays on the last
    // paint (often empty / still "loading") while messagesError is already set.
    if (config.positions[widgetId]?.activeChatId === chatId && telegramForumThreadId(chatId) === threadId) {
      updateTelegramWidget(card, widgetId);
    }
  } finally {
    if (generation === telegramRuntime.accountGeneration) telegramRuntime.loadingMessages.delete(stateKey);
  }
}

// Explicit mark-read (backend no longer auto-marks on fetch).
// Only the currently open chat whose messages are actually loaded may be marked —
// never a background/stale chat from a list refresh path.
// Deduped by newest message id (not wall-clock): a pure time throttle used to
// skip viewMessages for messages that arrived while the thread was already open,
// so the widget looked "read" but Telegram servers (other devices) never got the
// receipt.
function markTelegramChatRead(card, chatId, { force = false } = {}) {
  if (!chatId || !telegramRuntime.status?.authorized || !card?.classList.contains("expanded")) return;
  if (card._tgSurfaceTransitioning || card._tgPreparing) return;
  const cacheKey = telegramMessageCacheKey(chatId);
  if (!force && !telegramRuntime.messagesLoaded.has(cacheKey)) return;
  const widgetId = card.dataset?.id;
  if (widgetId) {
    const activeId = config.positions?.[widgetId]?.activeChatId;
    if (activeId && activeId !== chatId) return;
  }

  // Optimistically clear unread count in runtime state and in DOM immediately
  const chatObj = (telegramRuntime.chats || []).find((c) => c.id === chatId);
  if (chatObj && chatObj.unread) {
    chatObj.unread = 0;
    const row = card.querySelector(`.tg-chat-row[data-chat-id="${CSS.escape(chatId)}"]`);
    if (row) {
      row.classList.remove("unread");
      row.querySelector(".tg-row-unread")?.remove();
    }
  }

  const messages = telegramRuntime.messages.get(cacheKey) || [];
  let newestId = "";
  let newestNum = 0;
  for (const message of messages) {
    const n = Number(message?.id);
    if (Number.isFinite(n) && n > newestNum) {
      newestNum = n;
      newestId = String(message.id);
    }
  }
  // Also consider the chat list's last_message id (backend field) when history
  // is empty but the row still has an unread last message.
  if (!newestId) {
    const chat = (telegramRuntime.chats || []).find((c) => c.id === chatId);
    const lastId = chat?.lastMessageId != null ? String(chat.lastMessageId) : "";
    const n = Number(lastId);
    if (lastId && Number.isFinite(n) && n > 0) {
      newestId = lastId;
      newestNum = n;
    }
  }
  if (!force && !newestId && !telegramRuntime.messagesLoaded.has(cacheKey)) return;
  const lastMarkedId = telegramRuntime.lastMarkedRead.get(chatId) || "";
  if (!force && newestId && lastMarkedId === newestId) return;
  if (newestId) telegramRuntime.lastMarkedRead.set(chatId, newestId);
  try {
    const openP = invoke("telegram_open_chat", { chatId: chatId });
    if (openP && typeof openP.then === "function") openP.catch(() => {});
    const p = invoke("telegram_mark_read", { chatId: chatId });
    if (p && typeof p.then === "function") p.catch(() => {});
  } catch (_) {}
}

function openTelegramChatSession(chatId) {
  const chat = telegramVisibleChats().find((c) => c.id === chatId);
  if (chat?.isForum) {
    loadForumTopics(chatId);
  }

  if (!chatId || !telegramRuntime.status?.authorized) return;
  try {
    const p = invoke("telegram_open_chat", { chatId: chatId });
    if (p && typeof p.then === "function") p.catch(() => {});
  } catch (_) {}
}

function closeTelegramChatSession(chatId) {
  try {
    const p = invoke("telegram_close_chat", { chatId: chatId || null || null });
    if (p && typeof p.then === "function") p.catch(() => {});
  } catch (_) {}
}

function telegramEventChatId(payload) {
  if (!payload || typeof payload !== "object") return "";
  if (payload.chatId != null && payload.chatId !== "") return String(payload.chatId);
  if (payload.chat_id != null && payload.chat_id !== "") return String(payload.chat_id);
  return "";
}

function telegramEventMessage(payload) {
  if (!payload || typeof payload !== "object") return null;
  // Wrapped events: { chatId, message } / { chatId, messageId, message } / …
  if (payload.message && typeof payload.message === "object") return payload.message;
  // Bare message object (telegram-message-new today).
  return payload;
}

function telegramPreviewFromMessage(message) {
  if (!message) return "";
  if (message.text) return String(message.text);
  if (message.sticker) return "Sticker";
  const kind = message.attachmentKind;
  if (kind === "photo") return message.attachment || "Photo";
  if (kind === "animation") return message.attachment || "GIF";
  if (kind === "video") return message.attachment || "Video";
  if (kind === "audio") return message.attachment || "Audio";
  if (kind === "voice") return "Voice message";
  if (message.attachment) return String(message.attachment);
  if (kind) return String(kind);
  return "";
}

// Update one chat's preview/time/unread in memory and return the chat object.
function touchTelegramChatFromMessage(chatId, message, { isOpen } = {}) {
  const chats = telegramRuntime.chats;
  if (!Array.isArray(chats) || !chatId || !message) return null;
  const idx = chats.findIndex((c) => c.id === chatId);
  if (idx < 0) return null;
  const chat = { ...chats[idx] };
  chat.preview = telegramPreviewFromMessage(message);
  if (message.time) chat.time = message.time;
  if (isOpen) {
    // Viewer is on this thread; unread will clear via mark-read.
  } else if (message.from !== "me") {
    chat.unread = (Number(chat.unread) || 0) + 1;
  }
  // Bump this chat toward the top of its favorite/non-favorite group.
  chats.splice(idx, 1);
  if (chat.favorite) {
    let insertAt = 0;
    while (insertAt < chats.length && chats[insertAt].favorite && (Number(chats[insertAt].order) || 0) > (Number(chat.order) || 0)) {
      insertAt += 1;
    }
    chats.splice(insertAt, 0, chat);
  } else {
    let insertAt = 0;
    while (insertAt < chats.length && chats[insertAt].favorite) insertAt += 1;
    chats.splice(insertAt, 0, chat);
  }
  return chat;
}

function patchTelegramChatRowDom(card, chat) {
  if (!card || !chat?.id) return;
  const body = card.querySelector(".widget-body");
  if (!body) return;
  const row = body.querySelector(`.tg-chat-row[data-chat-id="${CSS.escape(String(chat.id))}"]`);
  if (row) {
    const timeEl = row.querySelector(".tg-row-title small");
    if (timeEl) timeEl.textContent = formatTelegramTime(chat.time) || "";
    const previewEl = row.querySelector(".tg-row-preview");
    if (previewEl) previewEl.textContent = chat.preview || "";
    let unreadEl = row.querySelector(".tg-row-unread");
    if (chat.unread) {
      if (!unreadEl) {
        unreadEl = document.createElement("span");
        unreadEl.className = "tg-row-unread" + (chat.muted ? " muted" : "");
        row.appendChild(unreadEl);
      } else {
        unreadEl.classList.toggle("muted", !!chat.muted);
      }
      unreadEl.textContent = chat.unread > 99 ? "99+" : String(chat.unread);
    } else if (unreadEl) {
      unreadEl.remove();
    }

    // Reorder row in DOM if list is available and search is not filtering
    const list = body.querySelector(".tg-chat-list");
    if (list && !telegramRuntime.chatFilter && (telegramRuntime.chatListView === "main" && !chat.isArchived)) {
      const isPinned = !!chat.favorite;
      let targetBefore = null;
      if (isPinned) {
        targetBefore = list.firstElementChild;
      } else {
        const firstUnpinned = list.querySelector('.tg-chat-row:not([data-pinned="true"])');
        targetBefore = firstUnpinned;
      }
      if (targetBefore && targetBefore !== row) {
        const prevScroll = list.scrollTop;
        list.insertBefore(row, targetBefore);
        if (prevScroll > 0) list.scrollTop = prevScroll;
      } else if (!targetBefore && list.firstElementChild !== row) {
        const prevScroll = list.scrollTop;
        list.prepend(row);
        if (prevScroll > 0) list.scrollTop = prevScroll;
      }
    }
  }
  const railBtn = body.querySelector(`.tg-rail [data-chat-id="${CSS.escape(String(chat.id))}"]`);
  if (railBtn) {
    let badge = railBtn.querySelector("em");
    const mutedDot = railBtn.querySelector("i");
    if (chat.unread) {
      if (!badge) {
        badge = document.createElement("em");
        railBtn.appendChild(badge);
      } else {
        badge.classList.toggle("muted", !!chat.muted);
      }
      badge.textContent = chat.unread > 99 ? "99+" : String(chat.unread);
      mutedDot?.remove();
    } else if (badge) {
      badge.remove();
    }
    // Restore the muted-dot when the rail has unread cleared and the chat is
    // still muted; otherwise the indicator silently disappears until a full
    // re-render (kept symmetric with the badge logic above).
    if (!chat.unread && chat.muted && !railBtn.querySelector("i")) {
      const dot = document.createElement("i");
      railBtn.appendChild(dot);
    }
  }
  if (card._tgSectionCache) {
    card._tgSectionCache.list = null;
    card._tgSectionCache.rail = null;
  }
}

// Append if this chat's history is already loaded; dedupe by id.
function appendTelegramRuntimeMessage(chatId, message) {
  const threadId = telegramMessageThreadId(message, chatId);
  const cacheKey = telegramMessageCacheKey(chatId, threadId);
  if (!chatId || !message || !telegramRuntime.messagesLoaded.has(cacheKey)) {
    return { appended: false, duplicate: false };
  }
  const current = telegramRuntime.messages.get(cacheKey) || [];
  if (message.id != null && message.id !== "" && current.some((m) => m.id === message.id)) {
    return { appended: false, duplicate: true };
  }
  cacheTelegramMessages(chatId, [...current, message], threadId);
  return { appended: true, duplicate: false };
}

function replaceTelegramRuntimeMessage(chatId, oldMessageId, message) {
  if (!chatId || !message) return false;
  const threadId = telegramMessageThreadId(message, chatId);
  const cacheKey = telegramMessageCacheKey(chatId, threadId);
  const current = telegramRuntime.messages.get(cacheKey) || [];
  const idx = current.findIndex((m) => m.id === oldMessageId);
  if (idx < 0) {
    // Final id may already be present (race with updateNewMessage).
    if (message.id && current.some((m) => m.id === message.id)) return false;
    if (telegramRuntime.messagesLoaded.has(cacheKey)) {
      cacheTelegramMessages(chatId, [...current, message], threadId);
      return true;
    }
    return false;
  }
  const next = current.slice();
  next[idx] = message;
  // Drop a duplicate final-id row if message-new raced ahead of message-sent.
  if (message.id) {
    for (let i = next.length - 1; i >= 0; i--) {
      if (i !== idx && next[i].id === message.id) next.splice(i, 1);
    }
  }
  cacheTelegramMessages(chatId, next, threadId);
  return true;
}

function removeTelegramRuntimeMessages(chatId, messageIds) {
  if (!chatId || !Array.isArray(messageIds) || !messageIds.length) return false;
  const drop = new Set(messageIds.map(String));
  const base = String(chatId);
  const cacheKeys = [base, ...[...telegramRuntime.messages.keys()].filter((key) => String(key).startsWith(`${base}::`))];
  let changed = false;
  for (const key of cacheKeys) {
    const current = telegramRuntime.messages.get(key) || [];
    if (!current.length) continue;
    const next = current.filter((message) => !drop.has(String(message.id)));
    if (next.length === current.length) continue;
    const threadId = key.startsWith(`${base}::`) ? key.slice(base.length + 2) : null;
    cacheTelegramMessages(base, next, threadId);
    changed = true;
  }
  return changed;
}

function wireTelegramMessageRow(card, widgetId, row, chat) {
  if (!row) return;
  wireTelegramAnimationControls(row);
  row.querySelectorAll(".tg-spoiler").forEach((spoiler) => {
    spoiler.addEventListener("click", (e) => {
      e.stopPropagation();
      spoiler.classList.toggle("revealed");
    });
  });

  // Lightbox clicks for img.tg-media are handled via delegated listener on messageScroller

  // Safe media helpers with promise guards
  const safePlay = async (el) => {
    if (!el) return;
    try {
      el._playPromise = el.play();
      await el._playPromise;
    } catch (err) {
      if (err.name !== "AbortError") console.warn("[tg-media] play error:", err);
    } finally {
      el._playPromise = null;
    }
  };
  const safePause = async (el) => {
    if (!el) return;
    if (el._playPromise) {
      try { await el._playPromise; } catch (_) {}
    }
    if (!el.paused) el.pause();
  };

  // Wire custom video players
  row.querySelectorAll(".tg-video-wrap:not(.tg-video-note-wrap)").forEach((wrap) => {
    if (wrap.classList.contains("tg-media-pending")) return;
    const video = wrap.querySelector("video.tg-media-video-player");
    if (!video || video._tgWired) return;
    video._tgWired = true;
    video.defaultMuted = true;
    video.muted = true;

    const centerOverlay = wrap.querySelector(".tg-video-play-overlay");
    const btnToggle = wrap.querySelector(".tg-video-btn-toggle");
    const timeline = wrap.querySelector(".tg-video-timeline");
    const progressFill = wrap.querySelector(".tg-video-timeline-progress");
    const thumb = wrap.querySelector(".tg-video-timeline-thumb");
    const timeCur = wrap.querySelector(".tg-video-time-cur");
    const btnMute = wrap.querySelector(".tg-video-btn-mute");
    const btnFs = wrap.querySelector(".tg-video-btn-fullscreen");
    const timeContainer = wrap.querySelector(".tg-video-time");
    const totalSecs = Number(timeContainer?.dataset.total || 0);

    const syncPlayState = (playing) => {
      wrap.classList.toggle("playing", playing);
      [centerOverlay, btnToggle].forEach((btn) => {
        if (!btn) return;
        const iconPlay = btn.querySelector(".tg-video-icon-play");
        const iconPause = btn.querySelector(".tg-video-icon-pause");
        if (iconPlay) iconPlay.style.display = playing ? "none" : "block";
        if (iconPause) iconPause.style.display = playing ? "block" : "none";
      });
    };

    const togglePlay = (e) => {
      e?.stopPropagation();
      if (video.paused) {
        card?.querySelectorAll("video, audio").forEach((m) => {
          if (m !== video && !m.paused) safePause(m);
        });
        safePlay(video);
      } else {
        safePause(video);
      }
    };

    centerOverlay?.addEventListener("click", togglePlay);
    btnToggle?.addEventListener("click", togglePlay);
    video.addEventListener("click", togglePlay);

    video.addEventListener("play", () => syncPlayState(true));
    video.addEventListener("pause", () => syncPlayState(false));
    video.addEventListener("ended", () => {
      syncPlayState(false);
      if (progressFill) progressFill.style.transform = "scaleX(0)";
      if (thumb) thumb.style.transform = "translateX(0px)";
      if (timeCur) timeCur.textContent = "0:00";
    });

    // Scrubbing with pointer capture & isScrubbing guard
    let isScrubbing = false;
    let wasPlaying = false;
    let cachedRect = null;
    let rafSeekId = null;

    const updateScrub = (e) => {
      if (!cachedRect || cachedRect.width <= 0) return;
      const pct = Math.max(0, Math.min(1, (e.clientX - cachedRect.left) / cachedRect.width));
      const dur = video.duration || totalSecs || 1;
      if (progressFill) progressFill.style.transform = `scaleX(${pct})`;
      if (thumb) thumb.style.transform = `translateX(${pct * cachedRect.width}px)`;
      if (timeCur) timeCur.textContent = tgFormatDuration(pct * dur);

      if (rafSeekId) cancelAnimationFrame(rafSeekId);
      rafSeekId = requestAnimationFrame(() => {
        if ("fastSeek" in video) {
          video.fastSeek(pct * dur);
        } else {
          video.currentTime = pct * dur;
        }
      });
    };

    timeline?.addEventListener("pointerdown", (e) => {
      e.stopPropagation();
      if (e.button !== 0) return;
      timeline.setPointerCapture(e.pointerId);
      isScrubbing = true;
      wrap.classList.add("scrubbing");
      wasPlaying = !video.paused;
      if (wasPlaying) video.pause();
      cachedRect = timeline.getBoundingClientRect();
      updateScrub(e);
    });

    timeline?.addEventListener("pointermove", (e) => {
      if (!isScrubbing) return;
      updateScrub(e);
    });

    const endScrub = (e) => {
      if (!isScrubbing) return;
      isScrubbing = false;
      wrap.classList.remove("scrubbing");
      cachedRect = null;
      try {
        if (timeline.hasPointerCapture(e.pointerId)) {
          timeline.releasePointerCapture(e.pointerId);
        }
      } catch (_) {}
      if (rafSeekId) cancelAnimationFrame(rafSeekId);
      if (wasPlaying) safePlay(video);
    };

    timeline?.addEventListener("pointerup", endScrub);
    timeline?.addEventListener("pointercancel", endScrub);
    timeline?.addEventListener("lostpointercapture", endScrub);

    // Timeupdate (strictly guarded when scrubbing)
    let lastSec = -1;
    video.addEventListener("timeupdate", () => {
      if (isScrubbing) return;
      const dur = video.duration || totalSecs || 1;
      const cur = video.currentTime;
      const pct = Math.max(0, Math.min(1, cur / dur));
      if (progressFill) progressFill.style.transform = `scaleX(${pct})`;
      if (thumb && timeline) {
        const w = timeline.clientWidth || 200;
        thumb.style.transform = `translateX(${pct * w}px)`;
      }
      const sec = Math.floor(cur);
      if (sec !== lastSec) {
        lastSec = sec;
        if (timeCur) timeCur.textContent = tgFormatDuration(sec);
      }
    });

    // Mute toggle
    btnMute?.addEventListener("click", (e) => {
      e.stopPropagation();
      video.muted = !video.muted;
      const isMuted = video.muted || video.volume === 0;
      const volOn = btnMute.querySelector(".tg-video-icon-vol-on");
      const volMuted = btnMute.querySelector(".tg-video-icon-vol-muted");
      if (volOn) volOn.style.display = isMuted ? "none" : "block";
      if (volMuted) volMuted.style.display = isMuted ? "block" : "none";
    });

    // Fullscreen toggle
    btnFs?.addEventListener("click", async (e) => {
      e.stopPropagation();
      try {
        if (document.fullscreenElement === wrap) {
          await document.exitFullscreen();
        } else {
          await wrap.requestFullscreen();
        }
      } catch (err) {
        console.warn("[tg-video] fs error:", err);
      }
    });

    const onFsChange = () => {
      const isFs = document.fullscreenElement === wrap;
      wrap.classList.toggle("fullscreen", isFs);
      const fsEnter = btnFs?.querySelector(".tg-video-icon-fs-enter");
      const fsExit = btnFs?.querySelector(".tg-video-icon-fs-exit");
      if (fsEnter) fsEnter.style.display = isFs ? "none" : "block";
      if (fsExit) fsExit.style.display = isFs ? "block" : "none";
    };
    document.addEventListener("fullscreenchange", onFsChange);

    // Controls auto-hide
    let hideTimer = null;
    const activateControls = () => {
      wrap.classList.add("controls-active");
      if (hideTimer) clearTimeout(hideTimer);
      if (!video.paused && !isScrubbing) {
        hideTimer = setTimeout(() => {
          wrap.classList.remove("controls-active");
        }, 2500);
      }
    };
    wrap.addEventListener("pointermove", activateControls);
    wrap.addEventListener("pointerenter", activateControls);
    wrap.addEventListener("pointerleave", () => {
      if (!video.paused && !isScrubbing) {
        wrap.classList.remove("controls-active");
      }
    });
  });

  // Wire video note ("кружочки")
  row.querySelectorAll(".tg-video-note-wrap").forEach((wrap) => {
    if (wrap.classList.contains("tg-media-pending")) return;
    const video = wrap.querySelector("video.tg-video-note-player");
    if (!video || video._tgWired) return;
    video._tgWired = true;
    video.defaultMuted = true;
    video.muted = true;

    const ringProgress = wrap.querySelector(".ring-progress");
    const overlay = wrap.querySelector(".tg-video-note-overlay");
    const circumference = 524.65; // 2 * pi * 83.5
    if (ringProgress) {
      ringProgress.style.strokeDasharray = `${circumference}`;
      ringProgress.style.strokeDashoffset = `${circumference}`;
    }

    let noteRafId = null;
    const updateRing = () => {
      if (video.paused || video.ended) return;
      const dur = video.duration || 1;
      const progress = Math.min(1, Math.max(0, video.currentTime / dur));
      if (ringProgress) {
        ringProgress.style.strokeDashoffset = (circumference * (1 - progress)).toFixed(2);
      }
      noteRafId = requestAnimationFrame(updateRing);
    };

    wrap.addEventListener("click", (e) => {
      e.stopPropagation();
      if (video.paused || video.ended) {
        card?.querySelectorAll("video, audio").forEach((m) => {
          if (m !== video && !m.paused) safePause(m);
        });
        if (video.ended) video.currentTime = 0;
        video.muted = false; // unmute on explicit user click
        safePlay(video);
        if (noteRafId) cancelAnimationFrame(noteRafId);
        noteRafId = requestAnimationFrame(updateRing);
      } else {
        safePause(video);
        if (noteRafId) cancelAnimationFrame(noteRafId);
      }
    });

    video.addEventListener("play", () => {
      wrap.classList.add("playing");
      if (overlay) {
        const iconPlay = overlay.querySelector(".icon-play");
        const iconPause = overlay.querySelector(".icon-pause");
        const iconReplay = overlay.querySelector(".icon-replay");
        if (iconPlay) iconPlay.style.display = "none";
        if (iconPause) iconPause.style.display = "block";
        if (iconReplay) iconReplay.style.display = "none";
      }
    });

    video.addEventListener("pause", () => {
      wrap.classList.remove("playing");
      if (overlay) {
        const iconPlay = overlay.querySelector(".icon-play");
        const iconPause = overlay.querySelector(".icon-pause");
        const iconReplay = overlay.querySelector(".icon-replay");
        if (iconPlay) iconPlay.style.display = "block";
        if (iconPause) iconPause.style.display = "none";
        if (iconReplay) iconReplay.style.display = "none";
      }
    });

    video.addEventListener("ended", () => {
      wrap.classList.remove("playing");
      if (noteRafId) cancelAnimationFrame(noteRafId);
      if (ringProgress) ringProgress.style.strokeDashoffset = "0";
      if (overlay) {
        const iconPlay = overlay.querySelector(".icon-play");
        const iconPause = overlay.querySelector(".icon-pause");
        const iconReplay = overlay.querySelector(".icon-replay");
        if (iconPlay) iconPlay.style.display = "none";
        if (iconPause) iconPause.style.display = "none";
        if (iconReplay) iconReplay.style.display = "block";
      }
    });
  });

  // Voice & Audio Message Playback
  row.querySelectorAll(".tg-voice-card, .tg-audio-card").forEach((cardEl) => {
    if (cardEl.classList.contains("pending")) return;
    const audio = cardEl.querySelector("audio");
    if (!audio || audio._tgWired) return;
    audio._tgWired = true;

    const playBtn = cardEl.querySelector(".tg-voice-play-btn, .tg-audio-play-btn");
    const progressFill = cardEl.querySelector(".tg-voice-progress-fill, .tg-audio-track-fill");
    const durationLabel = cardEl.querySelector(".tg-voice-duration, .tg-audio-duration");
    const totalSecs = Number(durationLabel?.dataset.total || 0);

    playBtn?.addEventListener("click", (e) => {
      e.stopPropagation();
      if (audio.paused) {
        // Pause other active media
        card?.querySelectorAll("audio, video").forEach((m) => {
          if (m !== audio && !m.paused) m.pause();
        });
        audio.play().catch(() => {});
      } else {
        audio.pause();
      }
    });

    audio.addEventListener("play", () => cardEl.classList.add("playing"));
    audio.addEventListener("pause", () => cardEl.classList.remove("playing"));
    audio.addEventListener("ended", () => {
      cardEl.classList.remove("playing");
      if (progressFill) progressFill.style.width = "0%";
      if (durationLabel) durationLabel.textContent = totalSecs > 0 ? tgFormatDuration(totalSecs) : "0:00";
    });

    audio.addEventListener("timeupdate", () => {
      const duration = audio.duration || totalSecs;
      const pct = duration ? (audio.currentTime / duration) * 100 : 0;
      if (progressFill) progressFill.style.width = `${pct}%`;
      if (durationLabel) {
        durationLabel.textContent = `${tgFormatDuration(audio.currentTime)} / ${tgFormatDuration(duration)}`;
      }
    });

    // Waveform / track seeking
    const trackArea = cardEl.querySelector(".tg-voice-waveform, .tg-audio-track");
    trackArea?.addEventListener("click", (e) => {
      e.stopPropagation();
      const rect = trackArea.getBoundingClientRect();
      const pct = Math.max(0, Math.min(1, (e.clientX - rect.left) / rect.width));
      const duration = audio.duration || totalSecs;
      if (duration > 0) {
        audio.currentTime = pct * duration;
        if (progressFill) progressFill.style.width = `${pct * 100}%`;
        if (audio.paused) {
          card?.querySelectorAll("audio, video").forEach((m) => {
            if (m !== audio && !m.paused) m.pause();
          });
          audio.play().catch(() => {});
        }
      }
    });
  });

  // Download Action for all media / files
  row.querySelectorAll(".tg-attachment-load, .tg-attachment-download").forEach((btn) => {
    btn.addEventListener("click", async (e) => {
      e.stopPropagation();
      telegramRuntime.mediaFailed?.delete(String(btn.dataset.fileId));
      const isFileCard = btn.closest(".tg-file-card") !== null;
      const openAfter = btn.classList.contains("tg-attachment-download") && isFileCard;
      btn.classList.add("downloading");
      btn.closest(".tg-file-card, .tg-voice-card, .tg-audio-card, .tg-video-wrap, .tg-video-note-wrap")?.classList.add("downloading");
      try {
        await handleTelegramAttachmentDownload(
          card,
          widgetId,
          btn.dataset.chatId,
          btn.dataset.messageId,
          btn.dataset.fileId,
          openAfter,
        );
      } finally {
        btn.classList.remove("downloading");
        btn.closest(".tg-file-card, .tg-voice-card, .tg-audio-card, .tg-video-wrap, .tg-video-note-wrap")?.classList.remove("downloading");
      }
    });
  });

  // Open Action for downloaded files
  row.querySelectorAll(".tg-attachment-open").forEach((btn) => {
    btn.addEventListener("click", async (e) => {
      e.stopPropagation();
      const path = btn.dataset.path || btn.closest("[data-path]")?.dataset.path;
      if (!path) return;
      const opened = await (async () => {
        try {
          await invoke("open_explorer", { path: path });
        } catch {
          await openPath(path);
        }
        return true;
      })();
      if (opened === false && chat?.id) {
        telegramRuntime.messagesError.set(telegramMessageCacheKey(chat.id), "Could not open file (blocked by app permissions)");
        updateTelegramWidget(card, widgetId);
      }
    });
  });
}

function wireTelegramAnimationControls(root) {
  root?.querySelectorAll?.(".tg-animation-wrap").forEach((wrap) => {
    const video = wrap.querySelector("video.tg-animation");
    const button = wrap.querySelector(".tg-animation-toggle");
    if (!video || !button || video._tgAnimationWired) return;
    video.defaultMuted = true;
    video.muted = true;
    const key = wrap.dataset.animationKey || "";
    video._tgUserPaused = telegramRuntime.pausedAnimations.has(key);
    const setButtonState = (paused) => {
      button.setAttribute("aria-label", paused ? "Play animation" : "Pause animation");
      button.innerHTML = paused
        ? '<svg viewBox="0 0 24 24" width="13" height="13" fill="currentColor"><path d="M8 5v14l11-7z"/></svg>'
        : '<svg viewBox="0 0 24 24" width="13" height="13" fill="currentColor"><path d="M6 5h4v14H6zm8 0h4v14h-4z"/></svg>';
      wrap.classList.toggle("animation-paused", paused);
    };
    setButtonState(video._tgUserPaused);
    button.addEventListener("click", (event) => {
      event.stopPropagation();
      if (video._tgUserPaused) {
        telegramRuntime.pausedAnimations.delete(key);
        video._tgUserPaused = false;
        setButtonState(false);
        video.play().catch(() => {});
      } else {
        telegramRuntime.pausedAnimations.add(key);
        video._tgUserPaused = true;
        video.pause();
        setButtonState(true);
      }
    });
    video._tgAnimationWired = true;
  });
}

function appendTelegramMessageDom(card, widgetId, chatId, message) {
  const scroller = card?.querySelector(".tg-messages");
  const threadId = telegramMessageThreadId(message, chatId);
  if (!scroller || (scroller.dataset.chatId || "") !== String(chatId)
      || String(scroller.dataset.threadId || "") !== String(threadId || "")) return false;
  const chat = (telegramRuntime.chats || []).find((c) => c.id === chatId)
    || telegramVisibleChats().find((c) => c.id === chatId);
  if (!chat) return false;
  scroller.querySelector(".tg-state")?.remove();
  const list = telegramRuntime.messages.get(telegramMessageCacheKey(chatId, threadId)) || [];
  if (telegramAlbumId(message)) {
    updateTelegramWidget(card, widgetId);
    return true;
  }
  const idx = list.findIndex((m) => m.id === message.id);
  const prev = idx > 0 ? list[idx - 1] : (list.length >= 2 ? list[list.length - 2] : null);
  const atLatest = scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight <= 36;
  const messageMap = new Map(list.map((m) => [String(m.id), m]));
  scroller.insertAdjacentHTML("beforeend", telegramMessageRowHtml(chat, message, prev, messageMap));
  const row = message.id
    ? scroller.querySelector(`.tg-message[data-message-id="${CSS.escape(String(message.id))}"]`)
    : scroller.querySelector(".tg-message:last-child");
  wireTelegramMessageRow(card, widgetId, row, chat);

  const messageRows = scroller.querySelectorAll(".tg-message");
  if (messageRows.length > 120) {
    const toRemove = messageRows.length - 100;
    for (let i = 0; i < toRemove; i++) {
      messageRows[i].remove();
    }
  }

  if (atLatest) {
    scroller.scrollTop = Math.max(0, scroller.scrollHeight - scroller.clientHeight);
    const downBtn = card.querySelector(".tg-scroll-down-btn");
    if (downBtn && !telegramRuntime.detachedState) {
      downBtn.classList.remove("visible");
      const badge = downBtn.querySelector(".tg-scroll-down-badge");
      if (badge) {
        badge.dataset.unreadCount = "0";
        badge.style.display = "none";
      }
    }
  } else {
    const downBtn = card.querySelector(".tg-scroll-down-btn");
    if (downBtn) {
      downBtn.classList.add("visible");
      const badge = downBtn.querySelector(".tg-scroll-down-badge");
      if (badge) {
        const cur = parseInt(badge.dataset.unreadCount || "0", 10) + 1;
        badge.dataset.unreadCount = String(cur);
        badge.textContent = cur > 99 ? "99+" : String(cur);
        badge.style.display = "inline-block";
      }
    }
  }
  if (card._tgSectionCache) card._tgSectionCache.thread = null;
  return true;
}

function replaceTelegramMessageDom(card, widgetId, chatId, oldMessageId, message) {
  const scroller = card?.querySelector(".tg-messages");
  const threadId = telegramMessageThreadId(message, chatId);
  if (!scroller || (scroller.dataset.chatId || "") !== String(chatId)
      || String(scroller.dataset.threadId || "") !== String(threadId || "")) return false;
  const chat = (telegramRuntime.chats || []).find((c) => c.id === chatId)
    || telegramVisibleChats().find((c) => c.id === chatId);
  if (!chat) return false;
  const selectorId = oldMessageId || message?.id;
  if (selectorId == null || selectorId === "") return false;
  const row = scroller.querySelector(`.tg-message[data-message-id="${CSS.escape(String(selectorId))}"]`);
  if (!row) return false;
  const list = telegramRuntime.messages.get(telegramMessageCacheKey(chatId, threadId)) || [];
  const idx = list.findIndex((m) => m.id === message.id || m.id === oldMessageId);
  const prev = idx > 0 ? list[idx - 1] : null;
  const messageMap = new Map(list.map((m) => [String(m.id), m]));
  row.outerHTML = telegramMessageRowHtml(chat, message, prev, messageMap);
  const newRow = scroller.querySelector(
    `.tg-message[data-message-id="${CSS.escape(String(message.id || oldMessageId))}"]`,
  );
  wireTelegramMessageRow(card, widgetId, newRow, chat);
  if (card._tgSectionCache) card._tgSectionCache.thread = null;
  return true;
}

function removeTelegramMessageDom(card, chatId, messageIds) {
  const scroller = card?.querySelector(".tg-messages");
  if (!scroller || (scroller.dataset.chatId || "") !== String(chatId)) return false;
  let removed = false;
  for (const id of messageIds || []) {
    const row = scroller.querySelector(`.tg-message[data-message-id="${CSS.escape(String(id))}"]`);
    if (row) {
      row.remove();
      removed = true;
    }
  }
  if (removed && card._tgSectionCache) card._tgSectionCache.thread = null;
  return removed;
}

function markTelegramMessageFailedDom(card, chatId, messageId, error) {
  const scroller = card?.querySelector(".tg-messages");
  if (!scroller || (scroller.dataset.chatId || "") !== String(chatId)) return false;
  const row = scroller.querySelector(`.tg-message[data-message-id="${CSS.escape(String(messageId))}"]`);
  if (!row) return false;
  row.classList.add("failed");
  if (!row.querySelector(".tg-send-failed")) {
    const tip = document.createElement("span");
    tip.className = "tg-send-failed";
    tip.textContent = error ? `failed to send: ${error}` : "failed to send";
    row.appendChild(tip);
  }
  if (card._tgSectionCache) card._tgSectionCache.thread = null;
  return true;
}

function handleTelegramMessageNewEvent(widgetId, payload) {
  if (payload?.generation != null && payload.generation !== telegramRuntime.accountGeneration) return;
  const message = telegramEventMessage(payload);
  if (!message) return;
  let chatId = telegramEventChatId(payload);
  if (!chatId && message.chatId != null && message.chatId !== "") chatId = String(message.chatId);
  if (!chatId && message.chat_id != null && message.chat_id !== "") chatId = String(message.chat_id);
  const activeId = config.positions?.[widgetId]?.activeChatId || "";
  const card = getTelegramCard();

  if (!chatId) {
    if (activeId && telegramRuntime.messagesLoaded.has(telegramMessageCacheKey(activeId))) {
      if (card) refreshTelegramMessages(card, widgetId, activeId, true);
    }
    return;
  }

  const isDetached = telegramRuntime.detachedState && String(telegramRuntime.detachedState.chatId) === String(chatId);
  if (isDetached) {
    if (!telegramRuntime.detachedState.pendingTailMessages.some((m) => m.id === message.id)) {
      telegramRuntime.detachedState.pendingTailMessages.push(message);
    }
    if (card) {
      const downBtn = card.querySelector(".tg-scroll-down-btn");
      if (downBtn) {
        downBtn.classList.add("visible");
        const badge = downBtn.querySelector(".tg-scroll-down-badge");
        if (badge) {
          const count = telegramRuntime.detachedState.pendingTailMessages.length;
          badge.textContent = count > 99 ? "99+" : String(count);
          badge.style.display = count > 0 ? "inline-block" : "none";
        }
      }
    }
    const chat = touchTelegramChatFromMessage(chatId, message, { isOpen: false });
    if (card && chat) patchTelegramChatRowDom(card, chat);
    return;
  }

  const { appended, duplicate } = appendTelegramRuntimeMessage(chatId, message);
  if (duplicate) return;

  const isOpen = activeId === chatId && card && (card.classList.contains("expanded") || card.classList.contains("active"));
  const chat = touchTelegramChatFromMessage(chatId, message, { isOpen: !!isOpen });
  if (card && chat) patchTelegramChatRowDom(card, chat);

  if (appended && isOpen && card) {
    const wasAppended = appendTelegramMessageDom(card, widgetId, chatId, message);
    if (wasAppended) {
      const scroller = card.querySelector(".tg-messages");
      const atLatest = scroller ? (scroller.scrollHeight - scroller.scrollTop - scroller.clientHeight <= 36) : false;
      if (atLatest) {
        markTelegramChatRead(card, chatId, { force: true });
      }
    }
  }
}

function handleTelegramMessageEditedEvent(widgetId, payload) {
  if (payload?.generation != null && payload.generation !== telegramRuntime.accountGeneration) return;
  const chatId = telegramEventChatId(payload);
  const messageId = payload?.messageId != null ? String(payload.messageId) : "";
  const message = telegramEventMessage(payload);
  if (!chatId || !message) return;
  const threadId = telegramMessageThreadId(message, chatId);
  const cacheKey = telegramMessageCacheKey(chatId, threadId);
  const current = telegramRuntime.messages.get(cacheKey) || [];
  const idx = current.findIndex((m) => m.id === messageId || m.id === message.id);
  if (idx < 0) return;
  const next = current.slice();
  next[idx] = message;
  cacheTelegramMessages(chatId, next, threadId);
  const card = getTelegramCard();
  const activeId = config.positions?.[widgetId]?.activeChatId || "";
  if (card && activeId === chatId) {
    replaceTelegramMessageDom(card, widgetId, chatId, messageId || message.id, message);
  }
}

function handleTelegramMessageReactionsEvent(widgetId, payload) {
  if (payload?.generation != null && payload.generation !== telegramRuntime.accountGeneration) return;
  const chatId = telegramEventChatId(payload);
  const messageId = payload?.messageId != null ? String(payload.messageId) : "";
  if (!chatId || !messageId) return;
  const cacheKey = telegramCachedMessageKey(chatId, messageId);
  const current = telegramRuntime.messages.get(cacheKey) || [];
  const idx = current.findIndex((m) => m.id === messageId);
  if (idx < 0) return;
  const next = current.slice();
  next[idx] = {
    ...next[idx],
    reactions: payload?.reactions ?? null,
    reactionItems: payload?.reactionItems ?? null,
  };
  const threadId = cacheKey.startsWith(`${chatId}::`) ? cacheKey.slice(String(chatId).length + 2) : null;
  cacheTelegramMessages(chatId, next, threadId);
  const card = getTelegramCard();
  const activeId = config.positions?.[widgetId]?.activeChatId || "";
  if (card && activeId === chatId) {
    const row = card.querySelector(`.tg-message[data-message-id="${CSS.escape(messageId)}"]`);
    const albumEl = row?.closest(".tg-album");
    if (albumEl) {
      let albumReactionsWrap = albumEl.querySelector(".tg-album-reactions");
      const rxHtml = renderTelegramReactionsHtml(next[idx], chatId);
      if (rxHtml) {
        if (!albumReactionsWrap) {
          albumReactionsWrap = document.createElement("div");
          albumReactionsWrap.className = "tg-album-reactions";
          albumEl.appendChild(albumReactionsWrap);
        }
        albumReactionsWrap.innerHTML = rxHtml;
      } else if (albumReactionsWrap) {
        albumReactionsWrap.remove();
      }
    } else {
      replaceTelegramMessageDom(card, widgetId, chatId, messageId, next[idx]);
    }
  }
}

function handleTelegramCustomEmojiReadyEvent(widgetId, payload) {
  const customEmojiId = payload?.customEmojiId != null ? String(payload.customEmojiId) : "";
  const path = payload?.path;
  if (!customEmojiId || !path) return;
  if (!telegramRuntime.customEmojiPaths) telegramRuntime.customEmojiPaths = new Map();
  telegramRuntime.customEmojiPaths.set(customEmojiId, path);

  const card = getTelegramCard();
  if (!card) return;
  const src = convertFileSrc ? convertFileSrc(path) : path;
  card.querySelectorAll(`[data-custom-emoji-id="${CSS.escape(customEmojiId)}"]`).forEach((el) => {
    if (el.tagName === "IMG") {
      el.src = src;
    } else {
      const img = document.createElement("img");
      img.className = "tg-rx-icon tg-rx-custom-img";
      img.src = src;
      img.alt = "";
      img.loading = "lazy";
      img.decoding = "async";
      img.dataset.customEmojiId = customEmojiId;
      el.replaceWith(img);
    }
  });
}

function handleTelegramMessageDeletedEvent(widgetId, payload) {
  if (payload?.generation != null && payload.generation !== telegramRuntime.accountGeneration) return;
  const chatId = telegramEventChatId(payload);
  const messageIds = Array.isArray(payload?.messageIds)
    ? payload.messageIds.map(String)
    : Array.isArray(payload?.message_ids)
      ? payload.message_ids.map(String)
      : [];
  if (!chatId || !messageIds.length) return;
  if (!removeTelegramRuntimeMessages(chatId, messageIds)) return;
  const card = getTelegramCard();
  const activeId = config.positions?.[widgetId]?.activeChatId || "";
  if (card && activeId === chatId) {
    removeTelegramMessageDom(card, chatId, messageIds);
    const threadId = telegramForumThreadId(chatId);
    const pinKey = threadId ? `${chatId}:${threadId}` : String(chatId);
    const currentPinnedId = telegramRuntime.pinnedMessages?.get(pinKey);
    if (currentPinnedId && messageIds.includes(String(currentPinnedId))) {
      telegramRuntime.pinnedMessages.delete(pinKey);
      telegramRuntime.pinnedMessageSnippet?.delete(pinKey);
      ensureTelegramChatPinnedMessage(chatId, threadId, true);
    }
  }
}

function handleTelegramMessagePinnedEvent(widgetId, payload) {
  if (payload?.generation != null && payload.generation !== telegramRuntime.accountGeneration) return;
  const chatId = telegramEventChatId(payload);
  const messageId = payload?.messageId != null ? String(payload.messageId) : "";
  const isPinned = payload?.isPinned === true;
  if (!chatId) return;

  const threadId = telegramForumThreadId(chatId);
  const pinKey = threadId ? `${chatId}:${threadId}` : String(chatId);

  const msgKey = telegramMessageCacheKey(chatId, threadId);
  const msgs = telegramRuntime.messages?.get(msgKey);
  if (Array.isArray(msgs)) {
    const found = msgs.find((m) => String(m.id) === messageId);
    if (found) found.isPinned = isPinned;
  }

  const card = getTelegramCard();
  const activeId = config.positions?.[widgetId]?.activeChatId || "";
  if (card && activeId === chatId) {
    ensureTelegramChatPinnedMessage(chatId, threadId, true);
  } else {
    telegramRuntime.pinnedMessages?.delete(pinKey);
    telegramRuntime.pinnedMessageSnippet?.delete(pinKey);
  }
}

function handleTelegramMessageSentEvent(widgetId, payload) {
  if (payload?.generation != null && payload.generation !== telegramRuntime.accountGeneration) return;
  const chatId = telegramEventChatId(payload);
  const oldMessageId = payload?.oldMessageId != null
    ? String(payload.oldMessageId)
    : payload?.old_message_id != null
      ? String(payload.old_message_id)
      : "";
  const message = telegramEventMessage(payload);
  if (!chatId || !message) return;
  replaceTelegramRuntimeMessage(chatId, oldMessageId, message);
  const card = getTelegramCard();
  const chat = touchTelegramChatFromMessage(chatId, message, {
    isOpen: config.positions?.[widgetId]?.activeChatId === chatId,
  });
  if (card && chat) patchTelegramChatRowDom(card, chat);
  const activeId = config.positions?.[widgetId]?.activeChatId || "";
  if (card && activeId === chatId) {
    if (!replaceTelegramMessageDom(card, widgetId, chatId, oldMessageId, message)) {
      // Temp row missing — append final if not already present in DOM.
      const scroller = card.querySelector(".tg-messages");
      const exists = message.id
        && scroller?.querySelector(`.tg-message[data-message-id="${CSS.escape(String(message.id))}"]`);
      if (!exists) appendTelegramMessageDom(card, widgetId, chatId, message);
    }
  }
}

function handleTelegramMessageSendFailedEvent(widgetId, payload) {
  if (payload?.generation != null && payload.generation !== telegramRuntime.accountGeneration) return;
  const chatId = telegramEventChatId(payload);
  const oldMessageId = payload?.oldMessageId != null
    ? String(payload.oldMessageId)
    : payload?.old_message_id != null
      ? String(payload.old_message_id)
      : "";
  const error = payload?.error != null ? String(payload.error) : "";
  if (!chatId || !oldMessageId) return;
  const failedKey = telegramCachedMessageKey(chatId, oldMessageId);
  const current = telegramRuntime.messages.get(failedKey) || [];
  const idx = current.findIndex((m) => m.id === oldMessageId);
  if (idx >= 0) {
    const next = current.slice();
    next[idx] = { ...next[idx], failed: true, sendError: error };
    const threadId = failedKey.startsWith(`${chatId}::`) ? failedKey.slice(String(chatId).length + 2) : null;
    cacheTelegramMessages(chatId, next, threadId);
  }
  const card = getTelegramCard();
  const activeId = config.positions?.[widgetId]?.activeChatId || "";
  if (card && activeId === chatId) {
    markTelegramMessageFailedDom(card, chatId, oldMessageId, error);
  }
}

// Global listeners for Tauri real-time events
let telegramEventsRegistered = false;

function setupTelegramListeners(card, widgetId) {
  if (telegramEventsRegistered) return;
  telegramEventsRegistered = true;
  // TDLib emits chat/file updates in bursts. Avatar completions only change the
  // chat rows; re-fetching the open message history for every such burst was
  // needless IPC/JSON/DOM work and made the entire shell hitch.
  let chatRefreshTimer = null;
  const scheduleChatRefresh = () => {
    clearTimeout(chatRefreshTimer);
    chatRefreshTimer = setTimeout(async () => {
      const currentCard = getTelegramCard();
      if (!currentCard) return;
      await refreshTelegramChats(currentCard, widgetId, true);
    }, 300);
  };
  listen("telegram-chat-update", () => {
    scheduleChatRefresh();
    if (telegramRuntime.chatListView === "archive") {
      clearTimeout(telegramRuntime.archiveRefreshTimer);
      telegramRuntime.archiveRefreshTimer = setTimeout(() => {
        const currentCard = getTelegramCard();
        if (currentCard) refreshTelegramArchiveChats(currentCard, widgetId, true);
      }, 300);
    }
  });
  // In-place message updates from TDLib (no full IPC round-trip / widget rebuild).
  listen("telegram-message-new", (event) => {
    handleTelegramMessageNewEvent(widgetId, event?.payload);
  });
  listen("telegram-message-edited", (event) => {
    handleTelegramMessageEditedEvent(widgetId, event?.payload);
  });
  listen("telegram-message-deleted", (event) => {
    handleTelegramMessageDeletedEvent(widgetId, event?.payload);
  });
  listen("telegram-message-pinned", (event) => {
    handleTelegramMessagePinnedEvent(widgetId, event?.payload);
  });
  listen("telegram-message-sent", (event) => {
    handleTelegramMessageSentEvent(widgetId, event?.payload);
  });
  listen("telegram-message-send-failed", (event) => {
    handleTelegramMessageSendFailedEvent(widgetId, event?.payload);
  });
  listen("telegram-message-reactions", (event) => {
    handleTelegramMessageReactionsEvent(widgetId, event?.payload);
  });
  listen("telegram-custom-emoji-ready", (event) => {
    handleTelegramCustomEmojiReadyEvent(widgetId, event?.payload);
  });
  listen("telegram-attachment-ready", (event) => {
    applyTelegramAttachmentReady(event?.payload);
  });
  // TDLib connection health (canonical states: ready / connecting /
  // connecting_to_proxy / updating / waiting_for_network). Surface a hint
  // instead of silently frozen messages, and when the connection returns,
  // refresh status so the UI catches up (debounced: a reconnect can produce
  // ready→updating→ready flaps).
  listen("telegram-connection-state", (event) => {
    const connState = event?.payload?.state;
    if (!connState) return;
    const prevConnState = telegramRuntime.connectionState;
    telegramRuntime.connectionState = connState;
    const currentCard = getTelegramCard();
    if (connState === "ready" && prevConnState !== "ready") {
      const now = Date.now();
      if (!telegramRuntime.lastReadyCatchupAt || now - telegramRuntime.lastReadyCatchupAt > 10000) {
        telegramRuntime.lastReadyCatchupAt = now;
        telegramRuntime.statusLoaded = false;
        if (currentCard) {
          refreshTelegramStatus(currentCard, widgetId);
          const activeId = config.positions?.[widgetId]?.activeChatId || "";
          if (activeId && telegramRuntime.messagesLoaded.has(telegramMessageCacheKey(activeId))) {
            refreshTelegramMessages(currentCard, widgetId, activeId, true);
          }
          telegramRuntime.chatsLoaded = false;
          refreshTelegramChats(currentCard, widgetId, true);
        }
      }
    }
    if (currentCard) {
      updateTelegramConnectionStrip(currentCard, connState);
    }
  });
  listen("telegram-user-status", (event) => {
    const { userId, status } = event?.payload || {};
    if (!userId) return;
    if (!telegramRuntime.userStatuses) telegramRuntime.userStatuses = new Map();
    telegramRuntime.userStatuses.set(String(userId), status);
    const card = getTelegramCard();
    if (!card) return;
    const activeId = config.positions?.[widgetId]?.activeChatId;
    const chat = (telegramRuntime.chats || []).find((c) => c.id === activeId);
    if (chat && String(chat.userId) === String(userId)) {
      const statusEl = card.querySelector(".tg-thread-head-info .tg-status-line");
      if (statusEl) {
        const chatStatus = formatTelegramChatStatus(chat);
        statusEl.className = `tg-status-line ${chatStatus.online ? 'tg-status-online' : ''}`;
        statusEl.innerHTML = `
          ${chatStatus.online ? '<span class="tg-online-dot"></span>' : ''}
          ${escapeHtml(chatStatus.text)}
        `;
      }
    }
  });
}

// Human hint for the TDLib connection strip; `ready` hides it.
function telegramConnectionText(state) {
  switch (state) {
    case "waiting_for_network":
      return "No network";
    case "connecting_to_proxy":
      return "Connecting via proxy…";
    case "connecting":
      return "Connecting…";
    case "updating":
      return "Syncing…";
    default:
      return "Connecting…";
  }
}

function telegramConnectionStripHtml() {
  const state = telegramRuntime.connectionState || "initializing";
  if (state === "ready" || state === "initializing") {
    return '<div class="tg-connection-state" style="display: none;"></div>';
  }
  return `<div class="tg-connection-state">${escapeHtml(telegramConnectionText(state))}</div>`;
}

function updateTelegramConnectionStrip(card, state) {
  const el = card.querySelector(".tg-connection-state");
  if (!el) return;
  if (!state || state === "ready") {
    el.textContent = "";
    el.style.display = "none";
    return;
  }
  el.textContent = telegramConnectionText(state);
  el.style.display = "block";
}

function telegramChatListState(chats) {
  if (telegramRuntime.chatListView === "archive") {
    if (telegramRuntime.archivedChatsError) {
      return `<div class="tg-state">Archive failed<br><small>${escapeHtml(telegramRuntime.archivedChatsError)}</small></div>`;
    }
    if (telegramRuntime.loadingArchivedChats && !telegramRuntime.archivedChatsLoaded) {
      return `<div class="tg-state loading">Loading archive</div>`;
    }
    if (telegramRuntime.archivedChatsLoaded && !chats.length) {
      return `<div class="tg-state">Archive is empty</div>`;
    }
  }
  if (telegramRuntime.statusLoaded && !telegramRuntime.status?.authorized) {
    return `<div class="tg-state">Sign in to load chats</div>`;
  }
  if (telegramRuntime.chatsError) {
    return `<div class="tg-state">Chat sync failed<br><small>${escapeHtml(telegramRuntime.chatsError)}</small></div>`;
  }
  if (telegramRuntime.loadingChats && !telegramRuntime.chatsLoaded) {
    return telegramSkeletonRows(10);
  }
  if (telegramRuntime.chatsLoaded && !chats.length) {
    return `<div class="tg-state">No chats yet</div>`;
  }
  return "";
}

function buildAccountSwitcherHtml(accounts, activeAccountId) {
  if (!accounts || !accounts.length) return "";
  const active = accounts.find((a) => a.id === activeAccountId) || accounts[0];
  const name = active?.name || "Account";
  return `
    <div class="tg-account-switcher">
      <button type="button" class="tg-account-btn" aria-label="Switch account">
        <span class="tg-account-btn-name">${escapeHtml(name)}</span>
        <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
          <path d="M6 9l6 6 6-6"/>
        </svg>
      </button>
      <div class="tg-account-dropdown">
        <div class="tg-account-dropdown-list">
          ${accounts.map((acc) => `
            <div class="tg-account-item ${acc.id === activeAccountId ? "active" : ""}" data-acc-id="${escapeHtml(acc.id)}">
              <div class="tg-account-item-info">
                <span class="tg-account-item-name">${escapeHtml(acc.name)}</span>
                ${acc.phone ? `<small class="tg-account-item-sub">${escapeHtml(acc.phone)}</small>` : acc.username ? `<small class="tg-account-item-sub">@${escapeHtml(acc.username)}</small>` : ""}
              </div>
              ${acc.id === activeAccountId ? '<span class="tg-account-check">✓</span>' : `
                <button type="button" class="tg-account-remove-btn" data-acc-id="${escapeHtml(acc.id)}" aria-label="Remove account">&times;</button>
              `}
            </div>
          `).join("")}
        </div>
        <button type="button" class="tg-account-add-btn">
          <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
            <line x1="12" y1="5" x2="12" y2="19"></line>
            <line x1="5" y1="12" x2="19" y2="12"></line>
          </svg>
          Add Account
        </button>
      </div>
    </div>
  `;
}

function buildRailHtml(chats, activeId) {
  const status = telegramRuntime.status;
  const showRailStatus = chats.length === 0 && (!status || !status.authorized);
  return `
        ${chats.slice(0, TELEGRAM_RAIL_CHAT_LIMIT).map((chat) => `
          <button type="button" class="tg-avatar ${chat.id === activeId ? "active" : ""} ${chat.favorite ? "favorite" : ""}" data-chat-id="${escapeHtml(chat.id)}" data-initials="${escapeHtml(chat.initials || (chat.title || "?").charAt(0))}" aria-label="${escapeHtml(chat.title)}">
            ${telegramAvatarInner(chat)}
            ${chat.unread ? `<em class="${chat.muted ? "muted" : ""}">${chat.unread > 99 ? "99+" : chat.unread}</em>` : chat.muted ? "<i></i>" : ""}
          </button>
        `).join("")}
        ${showRailStatus ? `
          <button type="button" class="tg-rail-status" aria-label="Telegram: sign in required">
            <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
              <path d="M21 4L3 11.5l7 2.4L13 20l3-14-7.2 6.8"/>
            </svg>
          </button>
        ` : ""}
      `;
}

function buildChatListHtml(chats, activeId, listState) {
  if (listState) return listState;
  const rows = telegramRenderedChats(chats);
  return rows.map((chat) => `
    <button type="button" class="tg-chat-row ${chat.id === activeId ? "active" : ""} ${chat.muted ? "muted" : ""}" data-chat-id="${escapeHtml(chat.id)}" data-pinned="${chat.favorite ? "true" : "false"}" data-initials="${escapeHtml(chat.initials || (chat.title || "?").charAt(0))}">
      <span class="tg-row-avatar" data-initials="${escapeHtml(chat.initials || (chat.title || "?").charAt(0))}">${telegramAvatarInner(chat)}</span>
      <span class="tg-row-main">
        <span class="tg-row-title">
          <span class="tg-row-title-text">${escapeHtml(chat.title)}</span>
          ${chat.favorite ? `<span class="tg-row-pin"><svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round"><line x1="12" y1="17" x2="12" y2="22"></line><path d="M5 17h14l-2-7V4h1V2H6v2h1v6l-2 7z"></path></svg></span>` : ""}
          <small>${escapeHtml(formatTelegramTime(chat.time))}</small>
        </span>
        <span class="tg-row-preview">${escapeHtml(chat.preview || "")}</span>
      </span>
      ${chat.unread ? `<span class="tg-row-unread${chat.muted ? " muted" : ""}">${chat.unread > 99 ? "99+" : chat.unread}</span>` : ""}
    </button>
  `).join("");
}

function buildChatListToolsHtml() {
  const isArchive = telegramRuntime.chatListView === "archive";
  return `
    <div class="tg-chat-tools">
      <input class="tg-chat-search" type="text" aria-label="Поиск чатов" placeholder="Search" value="${escapeHtml(telegramRuntime.chatFilter || "")}" />
      <button type="button" class="tg-archive-toggle${isArchive ? " active" : ""}" data-tg-action="toggle-archive" aria-label="${isArchive ? "Back to chats" : "Open archive"}" aria-pressed="${isArchive}">
        <svg viewBox="0 0 24 24" width="15" height="15" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
          ${isArchive
            ? '<path d="M19 12H5M12 19l-7-7 7-7"/>'
            : '<path d="M3 4h18v4H3zM5 8v12h14V8M9 12h6M9 16h6"/>'}
        </svg>
      </button>
    </div>
  `;
}

/** Build forum topics/subchats list view (shown in the 200px column). */
function buildForumTopicsListHtml(chat) {
  if (!chat) return "";
  const topics = telegramRuntime.forumTopics.get(chat.id) || [];
  const loading = telegramRuntime.forumTopicsLoading.has(chat.id);
  const activeWidgetId = getTelegramCard()?.dataset?.id || "telegram";
  const activeChatId = config.positions?.[activeWidgetId]?.activeChatId;
  const filter = telegramRuntime.forumFilterByChat.get(chat.id) || '';
  
  const headHtml = `
    <div class="tg-topics-header">
      <button type="button" class="tg-topics-back-btn" data-action="back-to-chats" aria-label="Назад к чатам">
        <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2">
          <path d="M19 12H5M12 19l-7-7 7-7"/>
        </svg>
        <span>Все чаты</span>
      </button>
      <div class="tg-topics-chat-title">${escapeHtml(chat.title)}</div>
      <input class="tg-topic-search" type="search" aria-label="Поиск тем" placeholder="Поиск тем" value="${escapeHtml(filter)}" />
    </div>
  `;

  if (loading && topics.length === 0) {
    return headHtml + `<div class="tg-state loading">Загрузка тем...</div>`;
  }
  if (topics.length === 0) {
    return headHtml + `<div class="tg-state">Нет тем</div>`;
  }
  const rows = topics.map((topic) => {
    const isActive = String(topic.id) === String(telegramForumThreadId(chat.id))
      || (topic.subchatId && String(topic.subchatId) === String(activeChatId));
    return `
      <button type="button" class="tg-topic-row${isActive ? " active" : ""}"
        aria-label="${escapeHtml(topic.title || "Topic")}"
        aria-pressed="${!!isActive}"
        data-chat-id="${escapeHtml(chat.id)}"
        data-topic-id="${escapeHtml(topic.id)}"
        ${filter.trim() && !(topic.title || '').toLocaleLowerCase().includes(filter.trim().toLocaleLowerCase()) ? 'hidden' : ''}
        ${topic.subchatId ? `data-subchat-id="${escapeHtml(topic.subchatId)}"` : ""}>
        <span class="tg-row-avatar tg-topic-icon">
          <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8">
            <path d="M4 9h16M4 15h16M10 3L8 21M16 3l-2 21"/>
          </svg>
        </span>
        <span class="tg-row-main">
          <span class="tg-row-title">
            <span class="tg-row-title-text">${escapeHtml(topic.title || "Topic")}</span>
          </span>
          <span class="tg-row-preview">${escapeHtml(topic.lastMessage || "")}</span>
        </span>
        ${topic.unreadCount ? `<span class="tg-row-unread">${topic.unreadCount > 99 ? "99+" : topic.unreadCount}</span>` : ""}
      </button>
    `;
  }).join("");
  return headHtml + `<div class="tg-topics-scroll">${rows}</div><div class="tg-state tg-topic-empty" hidden>Темы не найдены</div>`;
}

function renderTelegramReactionsHtml(message, chatId) {
  if (!message) return "";
  const items = message.reactionItems;
  if (Array.isArray(items) && items.length > 0) {
    return `<div class="tg-reactions">` + items.map((item) => {
      const isCustom = item.reactionType === "customEmoji" || Boolean(item.customEmojiId);
      const count = Number(item.count || 0);
      const isChosen = Boolean(item.isChosen);
      const customEmojiId = item.customEmojiId ? String(item.customEmojiId) : "";

      let iconHtml = "";
      if (isCustom && customEmojiId) {
        let path = item.customEmojiPath;
        if (!path && telegramRuntime.customEmojiPaths) {
          path = telegramRuntime.customEmojiPaths.get(customEmojiId);
        }
        if (path) {
          if (!telegramRuntime.customEmojiPaths) telegramRuntime.customEmojiPaths = new Map();
          telegramRuntime.customEmojiPaths.set(customEmojiId, path);
          const src = convertFileSrc ? convertFileSrc(path) : path;
          iconHtml = `<img class="tg-rx-icon tg-rx-custom-img" src="${escapeHtml(src)}" alt="" loading="lazy" decoding="async" data-custom-emoji-id="${escapeHtml(customEmojiId)}" />`;
        } else {
          if (!telegramRuntime.pendingCustomEmojiFetches) telegramRuntime.pendingCustomEmojiFetches = new Set();
          if (!telegramRuntime.pendingCustomEmojiFetches.has(customEmojiId)) {
            telegramRuntime.pendingCustomEmojiFetches.add(customEmojiId);
            invoke("telegram_get_custom_emoji_path", { customEmojiId }).then((fetchedPath) => {
              if (fetchedPath) {
                if (!telegramRuntime.customEmojiPaths) telegramRuntime.customEmojiPaths = new Map();
                telegramRuntime.customEmojiPaths.set(customEmojiId, fetchedPath);
                const card = getTelegramCard();
                if (card) {
                  const src = convertFileSrc ? convertFileSrc(fetchedPath) : fetchedPath;
                  card.querySelectorAll(`[data-custom-emoji-id="${CSS.escape(customEmojiId)}"]`).forEach((el) => {
                    if (el.tagName === "IMG") {
                      el.src = src;
                    } else {
                      const img = document.createElement("img");
                      img.className = "tg-rx-icon tg-rx-custom-img";
                      img.src = src;
                      img.alt = "";
                      img.loading = "lazy";
                      img.decoding = "async";
                      img.dataset.customEmojiId = customEmojiId;
                      el.replaceWith(img);
                    }
                  });
                }
              }
            }).catch(() => false);
          }
          iconHtml = `<span class="tg-rx-icon tg-rx-placeholder" data-custom-emoji-id="${escapeHtml(customEmojiId)}">✨</span>`;
        }
      } else {
        const emojiChar = item.emoji || "👍";
        iconHtml = `<span class="tg-rx-icon tg-rx-emoji">${escapeHtml(emojiChar)}</span>`;
      }

      return `
        <button type="button" class="tg-rx-pill ${isChosen ? "active" : ""}" 
          data-rx-chat-id="${escapeHtml(chatId || "")}" 
          data-message-id="${escapeHtml(message.id || "")}" 
          ${item.emoji ? `data-emoji="${escapeHtml(item.emoji)}"` : ""} 
          ${customEmojiId ? `data-custom-emoji-id="${escapeHtml(customEmojiId)}"` : ""} 
          data-reaction-type="${escapeHtml(item.reactionType || (isCustom ? "customEmoji" : "emoji"))}">
          ${iconHtml}
          ${count > 0 ? `<span class="tg-rx-count">${count}</span>` : ""}
        </button>
      `;
    }).join("") + `</div>`;
  }

  // Fallback: parse plain string if reactionItems is missing
  if (message.reactions && typeof message.reactions === "string") {
    const parts = message.reactions.trim().split(/\s+/);
    const pills = [];
    for (let i = 0; i < parts.length; i += 2) {
      const sym = parts[i];
      const cnt = parts[i + 1] || "";
      if (sym) {
        const isCustomPlaceholder = sym === "custom";
        const iconHtml = isCustomPlaceholder
          ? `<span class="tg-rx-icon tg-rx-placeholder">✨</span>`
          : `<span class="tg-rx-icon tg-rx-emoji">${escapeHtml(sym)}</span>`;
        pills.push(`
          <button type="button" class="tg-rx-pill" 
            data-rx-chat-id="${escapeHtml(chatId || "")}" 
            data-message-id="${escapeHtml(message.id || "")}" 
            ${!isCustomPlaceholder ? `data-emoji="${escapeHtml(sym)}"` : ""}
            data-reaction-type="${isCustomPlaceholder ? "customEmoji" : "emoji"}">
            ${iconHtml}
            ${cnt ? `<span class="tg-rx-count">${escapeHtml(cnt)}</span>` : ""}
          </button>
        `);
      }
    }
    if (pills.length) {
      return `<div class="tg-reactions">${pills.join("")}</div>`;
    }
  }

  return "";
}

// Single message bubble markup (shared by full thread builds and in-place patches).
function telegramMessageRowHtml(chat, message, prevMessage, messageMap, isAlbumMember = false) {
  if (!message) return "";
  const mediaOnly = !message.text
    && (message.attachmentKind === "photo" || message.attachmentKind === "animation" || message.attachmentKind === "video" || message.attachmentKind === "videoNote");
  const stickerOnly = !message.sticker && !message.text && !message.attachmentKind;
  // Emoji-only text (up to 3 emoji) mirrors Telegram: big glyph, no bubble.
  const bigEmoji = !message.attachmentKind && !message.sticker
    && tgIsEmojiOnly(message.text) && tgGraphemeCount(message.text) <= 3;
  const emojiCount = bigEmoji ? Math.min(tgGraphemeCount(message.text), 3) : 0;
  // Channel posts can carry an author signature even when TDLib reports the
  // channel itself as sender. Prefer that display name and retain sender names
  // for ordinary groups/channels as a fallback.
  const authorSignature = message.authorSignature || message.author_signature || "";
  const authorLabel = authorSignature || message.from || "";
  const previousAuthor = prevMessage?.authorSignature || prevMessage?.author_signature || prevMessage?.from || "";
  const showFrom = (chat.kind === "group" || chat.kind === "channel")
    && authorLabel && authorLabel !== "me"
    && (!prevMessage || previousAuthor !== authorLabel);
  const failed = !!(message.failed || message.sendFailed);

  let replyToHtml = "";
  const rawRid = message.replyToMessageId;
  const rid = (rawRid !== null && rawRid !== undefined && rawRid !== 0 && rawRid !== "0") ? String(rawRid).trim() : "";
  if (rid) {
    let replied = messageMap?.get(rid)
      || (chat?.id && telegramRuntime.messages.get(telegramMessageCacheKey(chat.id))?.find((m) => String(m.id) === rid))
      || (telegramRuntime.replyCache?.get(rid));

    let replySender = "";
    let replySnippet = "";

    if (replied && !replied.notFound) {
      replySender = replied.from === "me" ? "Вы" : (replied.from || "Сообщение");
      if (replied.text) {
        replySnippet = replied.text;
      } else if (replied.attachmentKind === "photo") {
        replySnippet = "📷 Фото";
      } else if (replied.attachmentKind === "video") {
        replySnippet = "📹 Видео";
      } else if (replied.attachmentKind === "animation") {
        replySnippet = "GIF";
      } else if (replied.attachmentKind === "videoNote") {
        replySnippet = "⭕ Видеосообщение";
      } else if (replied.attachmentKind === "voice") {
        replySnippet = "🎤 Голосовое сообщение";
      } else if (replied.attachmentKind === "audio") {
        replySnippet = `🎵 ${replied.attachmentTitle || replied.attachment || "Аудио"}`;
      } else if (replied.attachmentKind === "document" || replied.attachment) {
        replySnippet = `📄 ${replied.attachment || "Файл"}`;
      } else if (replied.sticker) {
        replySnippet = "🎭 Стикер";
      } else {
        replySnippet = "Сообщение";
      }
    } else {
      replySender = message.replyToSender || "";
      replySnippet = message.replyToText || "";
      if (!replySnippet && chat?.id && rid && !telegramRuntime.pendingReplies?.has(rid) && !telegramRuntime.replyCache?.has(rid)) {
        (telegramRuntime.pendingReplies ||= new Set()).add(rid);
        invoke("telegram_get_message", { chatId: chat.id, messageId: rid }).then((msg) => {
          if (!msg) {
            (telegramRuntime.replyCache ||= new Map()).set(rid, { notFound: true });
            return;
          }
          (telegramRuntime.replyCache ||= new Map()).set(rid, msg);
          const elements = document.querySelectorAll(`.tg-msg-reply[data-reply-id="${CSS.escape(rid)}"]`);
          elements.forEach((el) => {
            const senderSpan = el.querySelector(".tg-msg-reply-from");
            const textSpan = el.querySelector(".tg-msg-reply-text");
            const fromName = msg.from === "me" ? "Вы" : (msg.from || "");
            const textContent = msg.text || (msg.attachmentKind ? `📎 ${msg.attachment || msg.attachmentKind}` : "Сообщение");
            if (textSpan) textSpan.textContent = textContent;
            if (fromName) {
              if (senderSpan) {
                senderSpan.textContent = fromName;
              } else {
                const s = document.createElement("span");
                s.className = "tg-msg-reply-from";
                s.textContent = fromName;
                el.prepend(s);
              }
            }
          });
        }).catch(() => {
          (telegramRuntime.replyCache ||= new Map()).set(rid, { notFound: true });
        });
      }
      if (!replySnippet) {
        replySnippet = "В ответ на сообщение";
      }
    }

    replyToHtml = `
      <div class="tg-msg-reply" data-reply-id="${escapeHtml(message.replyToMessageId)}">
        <span class="tg-msg-reply-bar"></span>
        <div class="tg-msg-reply-content">
          ${replySender ? `<span class="tg-msg-reply-from">${escapeHtml(replySender)}</span>` : ""}
          <span class="tg-msg-reply-text">${escapeHtml(replySnippet)}</span>
        </div>
      </div>
    `;
  }

  let inlineKbHtml = "";
  if (message.replyMarkup && message.replyMarkup["@type"] === "replyMarkupInlineKeyboard" && Array.isArray(message.replyMarkup.rows)) {
    inlineKbHtml = `
      <div class="tg-inline-keyboard">
        ${message.replyMarkup.rows.map((row) => `
          <div class="tg-inline-row">
            ${(row || []).map((btn) => {
              const btnType = btn.type?.["@type"] || "";
              const url = btn.type?.url || "";
              const data = btn.type?.data || "";
              const isUrl = btnType === "inlineKeyboardButtonTypeUrl";
              return `
                <button type="button" class="tg-inline-btn" data-btn-type="${escapeHtml(btnType)}" data-url="${escapeHtml(url)}" data-data="${escapeHtml(data)}" data-chat-id="${escapeHtml(chat.id)}" data-message-id="${escapeHtml(message.id)}">
                  <span>${escapeHtml(btn.text)}</span>
                  ${isUrl ? `<svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2"><path d="M18 13v6a2 2 0 01-2 2H5a2 2 0 01-2-2V8a2 2 0 012-2h6M15 3h6v6M10 14L21 3"/></svg>` : ""}
                </button>
              `;
            }).join("")}
          </div>
        `).join("")}
      </div>
    `;
  }

  const reactionsHtml = isAlbumMember ? "" : renderTelegramReactionsHtml(message, chat?.id);

  return `
        <div class="tg-message ${message.from === "me" ? "out" : "in"} ${mediaOnly ? "media-only" : ""} ${stickerOnly ? "sticker-only" : ""} ${bigEmoji ? "big-emoji" : ""} ${failed ? "failed" : ""}" data-message-id="${escapeHtml(message.id || "")}">
          ${showFrom ? `<span class="tg-msg-from">${escapeHtml(authorLabel)}</span>` : ""}
          ${replyToHtml}
          ${message.text ? `<span class="tg-text${bigEmoji ? " tg-big-emoji" : ""}"${bigEmoji ? ` data-count="${emojiCount}"` : ""}>${tgBuildFormattedHtml(message.text, message.entities)}</span>` : ""}
          ${telegramAttachmentHtml(message, chat?.id)}
          ${message.sticker ? `<b class="tg-sticker">${escapeHtml(message.sticker)}</b>` : ""}
          ${failed ? `<span class="tg-send-failed">${escapeHtml(message.sendError ? `failed to send: ${message.sendError}` : "failed to send")}</span>` : ""}
          ${reactionsHtml}
          <small>${escapeHtml(formatTelegramTime(message.time))}</small>
          ${inlineKbHtml}
        </div>
      `;
}

function telegramAlbumId(message) {
  const value = message?.mediaAlbumId ?? message?.media_album_id ?? message?.groupedId ?? message?.grouped_id;
  return value == null || value === "" || String(value) === "0" ? "" : String(value);
}

function renderTelegramMessageRows(chat, messages, messageMap) {
  const rows = [];
  for (let index = 0; index < messages.length;) {
    const message = messages[index];
    const albumId = telegramAlbumId(message);
    if (!albumId) {
      rows.push(telegramMessageRowHtml(chat, message, index > 0 ? messages[index - 1] : null, messageMap));
      index += 1;
      continue;
    }
    const first = index;
    while (index < messages.length
      && telegramAlbumId(messages[index]) === albumId
      && messages[index].from === message.from) index += 1;
    const members = messages.slice(first, index);
    if (members.length === 1) {
      rows.push(telegramMessageRowHtml(chat, message, first > 0 ? messages[first - 1] : null, messageMap));
      continue;
    }
    const direction = message.from === "me" ? "out" : "in";
    const memberRows = members.map((member, offset) => telegramMessageRowHtml(
      chat,
      member,
      first + offset > 0 ? messages[first + offset - 1] : null,
      messageMap,
      true,
    )).join("");
    const memberWithReactions = members.find((m) => (m.reactionItems && m.reactionItems.length > 0) || m.reactions);
    const albumReactionsHtml = memberWithReactions ? renderTelegramReactionsHtml(memberWithReactions, chat?.id) : "";
    rows.push(`<div class="tg-album ${direction}" data-album-id="${escapeHtml(albumId)}">${memberRows}${albumReactionsHtml ? `<div class="tg-album-reactions">${albumReactionsHtml}</div>` : ""}</div>`);
  }
  return rows.join("");
}

// Inner content of .tg-messages (load-earlier + bubbles / state). Used by both
// full thread builds and lightweight message-list updates so drafts/composer stay put.
function buildMessagesInnerHtml(chat, messages) {
  if (!chat) return "";
  const selectedThreadId = telegramForumThreadId(chat.id);
  // Keep each forum topic isolated from the general chat. A thread selection
  // must never fall back to the base chat cache, or it will render unrelated
  // messages and appear to freeze at the wrong scroll position.
  const listToRender = selectedThreadId && selectedThreadId !== "0"
    ? (telegramRuntime.messages.get(telegramMessageCacheKey(chat.id, selectedThreadId)) || [])
    : messages;
  const messageState = telegramMessageState(chat, listToRender);
  const stateKey = telegramMessageCacheKey(chat.id, selectedThreadId);
  const historyExhausted = telegramRuntime.historyExhausted;
  const showLoadEarlier =
    telegramRuntime.messagesLoaded.has(stateKey) &&
    listToRender.length < TELEGRAM_MAX_CACHED_MESSAGES &&
    listToRender.some((message) => message.id != null && message.id !== "" && Number.isFinite(Number(message.id))) &&
    !(historyExhausted && historyExhausted.has(stateKey));
  const messageMap = new Map(listToRender.map((m) => [String(m.id), m]));
  return `
      ${showLoadEarlier ? `<button type="button" class="tg-load-earlier">Earlier messages</button>` : ""}
      ${messageState || renderTelegramMessageRows(chat, listToRender, messageMap)}
    `;
}

function buildThreadHtml(active, messages, showAuth, status, showSettings) {
  return showAuth ? renderTelegramAuth(status, showSettings) : renderTelegramThread(active, messages);
}

function formatTelegramTime(timeStr) {
  if (!timeStr) return "";
  // Mock data and older persisted labels may still carry legacy CP1251 mojibake.
  // Normalize before returning non-timestamp labels verbatim.
  timeStr = repairMojibake(timeStr);
  if (timeStr === "today" || timeStr === "recent" || timeStr === "now") return timeStr;
  // Only treat all-digit strings as unix timestamps (mock times like "11:42"
  // must not be parseInt'd — parseInt("11:42", 10) === 11 → "Jan 1").
  if (!/^\d+$/.test(timeStr)) return timeStr;
  const ts = parseInt(timeStr, 10);
  if (isNaN(ts)) return timeStr;
  const date = new Date(ts * 1000);
  const now = new Date();
  if (date.toDateString() === now.toDateString()) {
    return date.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  } else {
    return date.toLocaleDateString([], { month: 'short', day: 'numeric' });
  }
}

function telegramMessageState(chat, messages) {
  const stateKey = telegramMessageCacheKey(chat.id);
  const error = telegramRuntime.messagesError.get(stateKey);
  if (error) {
    return `<div class="tg-state">Messages failed<br><small>${escapeHtml(error)}</small></div>`;
  }
  if (telegramRuntime.loadingMessages.has(stateKey) && !telegramRuntime.messagesLoaded.has(stateKey)) {
    return `<div class="tg-state loading">Loading messages</div>`;
  }
  if (telegramRuntime.messagesLoaded.has(stateKey) && !messages.length) {
    return `<div class="tg-state">No messages here</div>`;
  }
  return "";
}

function cancelActiveTelegramMorph(card) {
  if (!card) return;
  if (card._activeMorphAnim) {
    try { card._activeMorphAnim.cancel(); } catch {}
    card._activeMorphAnim = null;
  }
  if (card._activeMorphCard && card._activeMorphCard.parentNode) {
    card._activeMorphCard.remove();
  }
  card._activeMorphCard = null;
  card._tgTransitioning = false;
}

function telegramMorphToThread(card, widgetId, originEl, targetThreadHtml, onFinish) {
  cancelActiveTelegramMorph(card);
  const panel = card.querySelector('.tg-panel');
  const outgoing = card.querySelector('.tg-layout > .tg-thread');
  const incoming = document.createElement('div');
  incoming.className = outgoing?.className || 'tg-thread';
  incoming.innerHTML = targetThreadHtml;
  const commit = () => {
    if (typeof onFinish === 'function') onFinish(incoming);
  };
  if (!panel || !outgoing || !(originEl instanceof HTMLElement)) {
    commit();
    return;
  }

  // Read geometry once. A scrolled row may be only partly visible.
  const bounds = panel.getBoundingClientRect();
  const source = originEl.getBoundingClientRect();
  const target = outgoing.getBoundingClientRect();
  const viewport = originEl.closest('.tg-chat-list, .tg-topics-list, .tg-rail')?.getBoundingClientRect() || bounds;
  const left = Math.max(source.left, viewport.left, bounds.left);
  const top = Math.max(source.top, viewport.top, bounds.top);
  const right = Math.min(source.right, viewport.right, bounds.right);
  const bottom = Math.min(source.bottom, viewport.bottom, bounds.bottom);
  const radius = getComputedStyle(originEl).borderRadius;
  const insets = rect => `inset(${rect.top - bounds.top}px ${bounds.right - rect.right}px ${bounds.bottom - rect.bottom}px ${rect.left - bounds.left}px`;
  if (right <= left || bottom <= top || target.width <= 0 || prefersReducedMotion()) {
    commit();
    return;
  }

  const plane = document.createElement('div');
  plane.className = 'tg-surface-morph';
  plane.inert = true;
  plane.setAttribute('aria-hidden', 'true');
  incoming.style.cssText = `position:absolute;left:${target.left - bounds.left}px;top:${target.top - bounds.top}px;width:${target.width}px;height:${target.height}px;`;
  // A decorative copy bridges the clicked row to the reveal. It cannot own
  // focus, events or duplicate IDs; the real list remains available to click.
  const sourceCopy = originEl.cloneNode(true);
  sourceCopy.removeAttribute('id');
  sourceCopy.querySelectorAll('[id]').forEach(node => node.removeAttribute('id'));
  sourceCopy.style.cssText = `position:absolute;left:${source.left - bounds.left}px;top:${source.top - bounds.top}px;width:${source.width}px;height:${source.height}px;margin:0;box-sizing:border-box;`;
  plane.append(sourceCopy, incoming);
  panel.append(plane);
  const scroller = incoming.querySelector('.tg-messages');
  if (scroller) scroller.scrollTop = scroller.scrollHeight;
  const wasInert = outgoing.inert;
  if (outgoing.contains(document.activeElement)) document.activeElement.blur();
  outgoing.inert = true;
  card._tgTransitioning = true;
  card._activeMorphCard = plane;
  const generation = telegramRuntime.accountGeneration;
  const selectedId = config.positions[widgetId]?.activeChatId;
  const selectedThread = telegramForumThreadId(selectedId);
  const duration = calculateDuration(target.width, target.height, right - left, bottom - top);
  let finished = false;
  const finalize = completed => {
    if (finished) return;
    finished = true;
    const ownsTransition = card._activeMorphAnim === timeline;
    timeline.cleanup();
    if (!ownsTransition) { plane.remove(); return; }
    card._activeMorphAnim = null;
    card._activeMorphCard = null;
    card._tgTransitioning = false;
    outgoing.inert = wasInert;
    const stillSelected = generation === telegramRuntime.accountGeneration
      && selectedId === config.positions[widgetId]?.activeChatId
      && selectedThread === telegramForumThreadId(selectedId);
    if (completed && stillSelected && outgoing.isConnected) {
      incoming.removeAttribute('style');
      // Move the prepared scroller/media/composer rather than building them again.
      commit();
    }
    plane.remove();
    if (completed && stillSelected && card._tgPendingUpdate) {
      card._tgPendingUpdate = false;
      updateTelegramWidget(card, widgetId);
    }
  };
  const timeline = motionTimeline({ duration, durationIsScaled: true, easing: EASINGS.expoOut,
    onFinish: () => finalize(true), onCancel: () => finalize(false) });
  // The opaque plane grows out of the clicked row and covers the old thread,
  // which therefore never fades: there is no frame where the panel is empty.
  timeline.add(plane, [
    { clipPath: `${insets({ left, top, right, bottom })} round ${radius})` },
    { clipPath: `${insets(target)} round 0px)` },
  ]);
  timeline.add(sourceCopy, [{ opacity: 1 }, { opacity: 0, offset: 0.35 }, { opacity: 0 }], { easing: EASINGS.content });
  // Header stays tied to the moving boundary; messages and input follow it.
  timeline.add(incoming, [{ opacity: 0 }, { opacity: 1, offset: 0.5 }, { opacity: 1 }], { easing: EASINGS.content });
  for (const content of incoming.querySelectorAll('.tg-messages, .tg-composer')) {
    timeline.add(content, [
      { opacity: 0, transform: 'translateY(6px)' },
      { opacity: 0, transform: 'translateY(6px)', offset: 0.2 },
      { opacity: 1, transform: 'translateY(0px)' },
    ], { easing: EASINGS.content });
  }
  card._activeMorphAnim = timeline;
  timeline.play();
}

// Shared active-chat switch used by full render and section rewires.
function telegramSetActiveChat(card, widgetId, chatId, originEl = null) {
  if (!chatId) return;
  telegramRuntime.chatSwitchEpoch = (telegramRuntime.chatSwitchEpoch || 0) + 1;
  if (!config.positions[widgetId]) config.positions[widgetId] = {};
  const prevId = config.positions[widgetId].activeChatId;
  if (prevId && prevId !== chatId) {
    closeTelegramChatSession(prevId);
    telegramRuntime.lastMarkedRead.delete(prevId);
    if (telegramRuntime.detachedState) {
      telegramRuntime.detachedState = null;
    }
  }
  config.positions[widgetId].activeChatId = chatId;
  saveConfig();
  // Switching threads invalidates the composer's pending mention state.
  tgMention.entities = [];
  tgMention.open = false;
  tgMention.items = [];
  openTelegramChatSession(chatId);

  // Update active selection state immediately in chat list and rail (0ms latency feedback)
  card.querySelectorAll(".tg-chat-list .tg-chat-row").forEach((row) => {
    row.classList.toggle("active", row.dataset.chatId === chatId);
  });
  card.querySelectorAll(".tg-rail [data-chat-id]").forEach((btn) => {
    btn.classList.toggle("active", btn.dataset.chatId === chatId);
  });

  const isExpanded = card.classList.contains("expanded");
  if (!isExpanded) {
    // Dock selection only changes the highlighted avatar. Building a thread
    // here would block the following open animation on the same click.
    updateTelegramWidget(card, widgetId);
    return;
  }
  const panelEl = card.querySelector(".tg-panel");
  const oldThread = card.querySelector(".tg-thread");
  const canMorph = isExpanded && panelEl && oldThread && (originEl instanceof HTMLElement) && oldThread.offsetWidth > 0;

  const chats = telegramVisibleChats();
  const active = chats.find((c) => c.id === chatId) || { id: chatId, title: "Chat", initials: "?" };
  const msgKey = telegramMessageCacheKey(chatId);
  refreshTelegramMessages(card, widgetId, chatId, true);
  const activeMessages = telegramRuntime.messages.get(msgKey) || [];
  const showAuth = (config.positions[widgetId]?.showSettings === true) || shouldShowTelegramAuth(telegramRuntime.status);
  const threadInnerHtml = buildThreadHtml(active, activeMessages, showAuth, telegramRuntime.status, false);
  const preparedMessageHtml = showAuth ? null : buildMessagesInnerHtml(active, activeMessages);

  if (canMorph && !showAuth) {
    telegramMorphToThread(card, widgetId, originEl, threadInnerHtml, incoming => {
      const liveThread = card.querySelector('.tg-layout > .tg-thread');
      if (liveThread) {
        const scroller = incoming.querySelector('.tg-messages');
        const preparedScroll = scroller?.isConnected ? captureTelegramScroll(scroller) : null;
        liveThread.replaceWith(incoming);
        const threadId = telegramForumThreadId(chatId);
        const pinKey = threadId ? `${chatId}:${threadId}` : String(chatId);
        const currentPinnedId = telegramRuntime.pinnedMessages?.get(pinKey);
        if (currentPinnedId && currentPinnedId !== "0" && !incoming.querySelector(".tg-pinned-bar")) {
          const pinMsg = telegramRuntime.pinnedMessageSnippet?.get(pinKey);
          const snippet = formatTelegramPinnedSnippet(pinMsg);
          const barHtml = `
            <div class="tg-pinned-bar" data-pinned-id="${escapeHtml(currentPinnedId)}">
              <div class="tg-pinned-icon">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
                  <line x1="12" y1="17" x2="12" y2="22"></line>
                  <path d="M5 17h14l-2-7V4h1V2H6v2h1v6l-2 7z"></path>
                </svg>
              </div>
              <div class="tg-pinned-content">
                <span class="tg-pinned-label">Закрепленное сообщение</span>
                <span class="tg-pinned-snippet">${escapeHtml(snippet)}</span>
              </div>
              <button type="button" class="tg-pinned-unpin-btn" data-action="unpin" aria-label="Открепить сообщение">
                <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                  <line x1="18" y1="6" x2="6" y2="18"></line>
                  <line x1="6" y1="6" x2="18" y2="18"></line>
                </svg>
              </button>
            </div>
          `;
          incoming.querySelector(".tg-messages")?.insertAdjacentHTML("beforebegin", barHtml);
        }
        wireTelegramComposer(card, widgetId, incoming, active);
        if (scroller) {
          if (preparedScroll) restoreTelegramScroll(scroller, preparedScroll);
          else scroller.scrollTop = scroller.scrollHeight;
          settleTelegramScroll(scroller);
        }
        wireTelegramMessagesSection(card, widgetId, incoming, active, {
          restoreScroll: false, showAuth, contentOnly: false,
        });
        wireTelegramAvatarErrors(incoming.querySelector('.tg-thread-head'));
      }
      if (card._tgSectionCache) card._tgSectionCache.thread = preparedMessageHtml;
      markTelegramChatRead(card, chatId, { force: true });
    });
    // The refresh started before preparing the incoming history.
  } else {
    cancelActiveTelegramMorph(card);
    renderTelegramWidget(card, widgetId);
    markTelegramChatRead(card, chatId, { force: true });
  }
}

// Avatar <img> error → initials fallback (rail / list / thread head).
function wireTelegramAvatarErrors(root) {
  if (!root) return;
  // If an avatar <img> fails to load (bad path, protocol scope, temp file not ready),
  // fall back to initials without leaving a broken image icon.
  root.querySelectorAll(".tg-ava-img").forEach((img) => {
    img.addEventListener("error", () => {
      const container = img.closest(".tg-avatar, .tg-row-avatar, .tg-thread-avatar");
      const initials = (container && container.getAttribute("data-initials")) || "?";
      const failedSrc = img.currentSrc || img.src || "unknown";
      const failures = (telegramRuntime.avatarRenderFailures ||= new Set());
      // A blocked asset scope can fail every avatar at once. Keep enough unique
      // samples for diagnosis without turning thousands of <img> errors into
      // synchronous Tauri IPC + disk writes that stall the whole shell.
      if (!failures.has(failedSrc) && failures.size < 20) {
        failures.add(failedSrc);
        logFrontend(`telegram avatar: browser failed to render image for initials=${initials} src=${failedSrc}`);
      }
      if (img.parentNode) {
        img.replaceWith(Object.assign(document.createElement("span"), { textContent: initials }));
      }
    });
  });
}

function wireTelegramAccountSwitcher(card, widgetId, body, activeAccountId) {
  const switcher = body.querySelector(".tg-account-switcher");
  const accountBtn = body.querySelector(".tg-account-btn");
  const dropdown = switcher?.querySelector(".tg-account-dropdown");
  if (!accountBtn || !switcher || !dropdown) return;

  accountBtn.addEventListener("click", (e) => {
    e.stopPropagation();
    if (surfaceManager.isOpen(dropdown) || switcher.classList.contains("open")) {
      surfaceManager.closeSurface(dropdown);
    } else {
      surfaceManager.openSurface(dropdown, {
        origin: accountBtn,
        originRadius: "8px",
        elevation: "var(--z-surface-popup, 2500)",
        display: "flex",
        ignoreOutsideElements: [accountBtn],
        onOpen: () => {
          switcher.classList.add("open");
        },
        onClosed: () => {
          switcher.classList.remove("open");
        },
      });
    }
  });

  body.querySelectorAll(".tg-account-item").forEach((item) => {
    item.addEventListener("click", async (e) => {
      if (e.target.closest(".tg-account-remove-btn")) return;
      e.stopPropagation();
      const accId = item.dataset.accId;
      if (accId && accId !== activeAccountId) {
        surfaceManager.closeSurface(dropdown);
        switcher.classList.remove("open");
        const bodyEl = card.querySelector(".widget-body");
        if (bodyEl) bodyEl.style.opacity = "0.4";
        clearTelegramAccountState();
        if (config.positions[widgetId]) {
          config.positions[widgetId].activeChatId = null;
        }
        await saveConfig();
        try {
          await invoke("telegram_switch_account", { accountId: accId });
        } catch (err) {
          console.error("switch account failed:", err);
        }
        telegramRuntime.statusLoaded = false;
        telegramRuntime.chatsLoaded = false;
        await refreshTelegramStatus(card, widgetId);
        await refreshTelegramChats(card, widgetId, true);
        if (bodyEl) bodyEl.style.opacity = "";
      }
    });
  });

  body.querySelectorAll(".tg-account-remove-btn").forEach((btn) => {
    btn.addEventListener("click", async (e) => {
      e.stopPropagation();
      const accId = btn.dataset.accId;
      if (accId) {
        surfaceManager.closeSurface(dropdown);
        clearTelegramAccountState();
        if (config.positions[widgetId]) {
          config.positions[widgetId].activeChatId = null;
        }
        await saveConfig();
        try {
          await invoke("telegram_remove_account", { accountId: accId });
        } catch (err) {
          console.error("remove account failed:", err);
        }
        telegramRuntime.statusLoaded = false;
        refreshTelegramStatus(card, widgetId);
      }
    });
  });

  body.querySelector(".tg-account-add-btn")?.addEventListener("click", async (e) => {
    e.stopPropagation();
    surfaceManager.closeSurface(dropdown);
    switcher.classList.remove("open");
    try {
      await invoke("telegram_add_account");
    } catch (err) {
      console.error("start qr login failed:", err);
    }
    telegramRuntime.statusLoaded = false;
    refreshTelegramStatus(card, widgetId);
  });
}

function wireTelegramRailSection(card, widgetId, body) {
  const rail = body.querySelector(".tg-rail");
  if (!rail) return;
  rail.querySelector(".tg-rail-status")?.addEventListener("click", (e) => {
    e.stopPropagation();
    if (editMode) return;
    toggleTelegramWidget(card, widgetId);
  });
  rail.querySelectorAll("[data-chat-id]").forEach((item) => {
    item.addEventListener("click", (e) => {
      e.stopPropagation();
      if (editMode) return;
      telegramSetActiveChat(card, widgetId, item.dataset.chatId, item);
      if (!card.classList.contains("expanded")) toggleTelegramWidget(card, widgetId);
    });
  });
  wireTelegramAvatarErrors(rail);
}

function wireTelegramListSection(card, widgetId, body) {
  const listEl = body.querySelector(".tg-chat-list");
  if (!listEl) return;
  listEl.querySelectorAll("[data-chat-id]").forEach((item) => {
    item.addEventListener("click", (e) => {
      e.stopPropagation();
      if (editMode) return;
      // Topic row click → select topic and show its messages
      const subchatId = item.dataset.subchatId;
      if (subchatId) {
        telegramRuntime.forumOpenChatId = null;
        updateTelegramWidget(card, widgetId);
        telegramSetActiveChat(card, widgetId, subchatId, item);
        return;
      }
      const topicId = item.dataset.topicId;
      if (topicId) {
        const chatId = item.dataset.chatId;
        telegramRuntime.pendingScrollAnchor = null;
        const messageScroller = card.querySelector(".tg-messages");
        if (messageScroller) messageScroller._tgSuppressAutoLoad = false;
        setTelegramForumThread(chatId, topicId);
        const chats = telegramVisibleChats();
        const active = chats.find((c) => c.id === chatId);
        if (active) {
          telegramSetActiveChat(card, widgetId, chatId, item);
        }
        return;
      }
      // Back button from topics → clear topic selection and show chat list
      const action = item.dataset.action;
      if (action === "back-to-chats") {
        setTelegramForumThread(item.dataset.chatId, null);
        telegramRuntime.pendingScrollAnchor = null;
        updateTelegramWidget(card, widgetId);
        return;
      }
      const chat = telegramVisibleChats().find((c) => c.id === item.dataset.chatId);
      if (chat?.isForum || chat?.hasSubchats || chat?.linkedCommunityId) {
        telegramRuntime.forumOpenChatId = chat.id;
        loadForumTopics(chat.id);
        if (!card.classList.contains("expanded")) toggleTelegramWidget(card, widgetId);
        updateTelegramWidget(card, widgetId);
        return;
      }
      telegramRuntime.forumOpenChatId = null;
      updateTelegramWidget(card, widgetId);
      telegramSetActiveChat(card, widgetId, item.dataset.chatId, item);
      if (!card.classList.contains("expanded")) toggleTelegramWidget(card, widgetId);
    });
  });
  listEl.onscroll = () => {
    if (listEl._tgRestoringScroll) return;
    if ((telegramRuntime.chatFilter || "").trim()) return;
    const chats = telegramVisibleChats();
    if ((telegramRuntime.chatVisibleLimit || 0) >= chats.length) return;
    if (listEl.scrollHeight - listEl.scrollTop - listEl.clientHeight > 140) return;
    const previousTop = listEl.scrollTop;
    telegramRuntime.chatVisibleLimit = Math.min(
      chats.length,
      (telegramRuntime.chatVisibleLimit || TELEGRAM_CHAT_PAGE_SIZE) + TELEGRAM_CHAT_PAGE_SIZE,
    );
    const activeId = config.positions[widgetId]?.activeChatId;
    const nextList = buildChatListHtml(chats, activeId, telegramChatListState(chats));
    listEl.innerHTML = nextList;
    if (card._tgSectionCache) card._tgSectionCache.list = nextList;
    wireTelegramListSection(card, widgetId, body);
    listEl.scrollTop = previousTop;
    kickTelegramAvatarDownloads(card, widgetId, chats);
  };
  wireTelegramAvatarErrors(listEl);
}

/** Load forum topics for a chat and cache them. */
function loadForumTopics(chatId) {
  if (telegramRuntime.forumTopicsLoading.has(chatId)) return;
  if (telegramRuntime.forumTopics.has(chatId)) return;
  telegramRuntime.forumTopicsLoading.add(chatId);
  const generation = telegramRuntime.accountGeneration;
  
  invoke("telegram_get_forum_topics", { request: { chatId: chatId, query: null } }).then((res) => {
    if (generation !== telegramRuntime.accountGeneration) return;
    const rawTopics = res?.topics || [];
    const topics = rawTopics.map((t) => {
      const info = t.info || {};
      const subchatId = info.subchat_id || t.subchat_id;
      const threadId = info.forum_topic_id || info.message_thread_id || t.forum_topic_id || t.message_thread_id || t.id || subchatId;
      const title = info.name || t.name || t.title || "General";
      let lastText = "";
      if (t.last_message) {
        lastText = t.last_message.content?.text?.text || t.last_message.content?.caption?.text || "";
      }
      return {
        id: String(threadId || ""),
        title: title,
        lastMessage: lastText || t.lastMessage || "",
        unreadCount: t.unread_count || 0,
        subchatId: subchatId ? String(subchatId) : null,
      };
    });
    telegramRuntime.forumTopics.set(chatId, topics);
    const card = document.querySelector(".widget-card.telegram-widget.expanded");
    if (card) {
      const widgetId = card.dataset.id || "telegram";
      if (config.positions?.[widgetId]?.activeChatId === chatId || telegramRuntime.forumOpenChatId === chatId) {
        updateTelegramWidget(card, widgetId);
      }
    }
  }).catch(() => {
    if (generation === telegramRuntime.accountGeneration) telegramRuntime.forumTopics.set(chatId, []);
  }).finally(() => {
    if (generation === telegramRuntime.accountGeneration) {
      telegramRuntime.forumTopicsLoading.delete(chatId);
      const card = document.querySelector('.telegram-widget.expanded');
      if (card && telegramRuntime.forumOpenChatId === chatId) updateTelegramWidget(card, card.dataset.id);
    }
  });
}

/** Wire clicks/events for the forum topics column. */
function wireTelegramTopicsSection(card, widgetId, body, active) {
  const topicsEl = body.querySelector(".tg-topics-list");
  if (!topicsEl) return;
  const search = topicsEl.querySelector('.tg-topic-search');
  const filterTopics = () => {
    const query = (search?.value || '').trim().toLocaleLowerCase();
    let visible = 0;
    topicsEl.querySelectorAll('.tg-topic-row').forEach((row) => {
      row.hidden = !row.getAttribute('aria-label').toLocaleLowerCase().includes(query);
      if (!row.hidden) visible += 1;
    });
    const empty = topicsEl.querySelector('.tg-topic-empty');
    if (empty) empty.hidden = visible > 0;
  };
  search?.addEventListener('input', () => {
    const forumId = telegramRuntime.forumOpenChatId;
    if (forumId) telegramRuntime.forumFilterByChat.set(forumId, search.value);
    filterTopics();
  });
  filterTopics();

  // Back button: return to all chats
  topicsEl.querySelectorAll('[data-action="back-to-chats"]').forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      telegramRuntime.forumOpenChatId = null;
      // Returning to chats keeps the selected topic and its reading position.
      telegramRuntime.pendingScrollAnchor = null;
      updateTelegramWidget(card, widgetId);
    });
  });

  // Topic or Subchat item click
  topicsEl.querySelectorAll(".tg-topic-row").forEach((item) => {
    item.addEventListener("click", (e) => {
      e.stopPropagation();
      if (editMode) return;
      const subchatId = item.dataset.subchatId;
      if (subchatId) {
        telegramSetActiveChat(card, widgetId, subchatId, item);
        return;
      }
      const topicId = item.dataset.topicId;
      const chatId = item.dataset.chatId || active?.id;
      if (!topicId || !chatId) return;
      telegramRuntime.pendingScrollAnchor = null;
      const messageScroller = card.querySelector(".tg-messages");
      if (messageScroller) messageScroller._tgSuppressAutoLoad = false;
      setTelegramForumThread(chatId, topicId);
      telegramSetActiveChat(card, widgetId, chatId, item);
    });
  });
  wireTelegramAvatarErrors(topicsEl);
}

function teardownTelegramMedia(container) {
  if (!container) return;
  container.querySelectorAll("video, audio").forEach((media) => {
    try {
      media.pause();
      media.src = "";
      media.load();
    } catch {}
  });
}

async function jumpToTelegramMessage(card, widgetId, chatId, targetMessageId, originatingEl = null) {
  if (!card || !chatId || !targetMessageId) return;
  const token = ++telegramRuntime.jumpToken;
  const scroller = card.querySelector(".tg-messages");
  if (!scroller) return;

  const target = scroller.querySelector(`[data-message-id="${CSS.escape(String(targetMessageId))}"]`);
  if (target) {
    target.scrollIntoView({ behavior: prefersReducedMotion() ? "instant" : "smooth", block: "center" });
    target.classList.add("tg-highlight");
    setTimeout(() => target.classList.remove("tg-highlight"), 1400);
    return;
  }

  if (originatingEl && originatingEl.classList.contains("tg-pinned-bar")) {
    originatingEl.classList.add("is-loading");
  }

  const threadId = telegramForumThreadId(chatId);
  let aroundMessages = [];
  try {
    const res = await invoke("telegram_messages_around", {
      chatId: String(chatId),
      messageId: String(targetMessageId),
      threadId: threadId ? Number(threadId) || null : null,
      limit: 50,
    });
    if (Array.isArray(res)) {
      aroundMessages = res;
    }
  } catch (err) {
    logFrontend(`telegram jumpToMessage error: ${err}`);
    return;
  } finally {
    if (originatingEl) originatingEl.classList.remove("is-loading");
  }

  if (token !== telegramRuntime.jumpToken) return;
  const activeId = config.positions?.[widgetId]?.activeChatId;
  if (String(activeId) !== String(chatId) || telegramForumThreadId(chatId) !== threadId) return;
  if (!aroundMessages.length) return;

  const cacheKey = telegramMessageCacheKey(chatId, threadId);
  if (!telegramRuntime.detachedState || String(telegramRuntime.detachedState.chatId) !== String(chatId)) {
    const currentTail = telegramRuntime.messages.get(cacheKey) || [];
    telegramRuntime.detachedState = {
      chatId: String(chatId),
      threadId: threadId ? String(threadId) : null,
      tailMessages: currentTail.slice(),
      tailScrollTop: scroller.scrollTop,
      tailScrollHeight: scroller.scrollHeight,
      pendingTailMessages: [],
    };
  }

  teardownTelegramMedia(scroller);
  cacheTelegramMessages(chatId, aroundMessages, threadId);
  telegramRuntime.pendingScrollAnchor = {
    chatId: String(chatId),
    threadId: threadId ? String(threadId) : null,
    messageId: String(targetMessageId),
    align: "center",
  };

  if (card._tgSectionCache) card._tgSectionCache.thread = null;
  updateTelegramWidget(card, widgetId);

  const downBtn = card.querySelector(".tg-scroll-down-btn");
  if (downBtn) downBtn.classList.add("visible");

  requestAnimationFrame(() => {
    const nextScroller = card.querySelector(".tg-messages");
    if (!nextScroller) return;
    const targetEl = nextScroller.querySelector(`[data-message-id="${CSS.escape(String(targetMessageId))}"]`);
    if (targetEl) {
      targetEl.scrollIntoView({ behavior: prefersReducedMotion() ? "instant" : "smooth", block: "center" });
      targetEl.classList.add("tg-highlight");
      setTimeout(() => targetEl.classList.remove("tg-highlight"), 1400);
    }
  });
}

function restoreTelegramLiveTail(card, widgetId, chatId) {
  if (!card) return;
  const scroller = card.querySelector(".tg-messages");
  const detached = telegramRuntime.detachedState;

  if (!detached || (chatId && String(detached.chatId) !== String(chatId))) {
    if (scroller) {
      scroller.scrollTo({ top: scroller.scrollHeight, behavior: prefersReducedMotion() ? "instant" : "smooth" });
    }
    const downBtn = card.querySelector(".tg-scroll-down-btn");
    if (downBtn) downBtn.classList.remove("visible");
    return;
  }

  telegramRuntime.detachedState = null;
  const targetChatId = detached.chatId;
  const threadId = detached.threadId;

  const merged = detached.tailMessages.slice();
  for (const msg of (detached.pendingTailMessages || [])) {
    if (!merged.some((m) => m.id === msg.id)) {
      merged.push(msg);
    }
  }

  cacheTelegramMessages(targetChatId, merged, threadId);

  if (scroller) teardownTelegramMedia(scroller);

  telegramRuntime.pendingScrollAnchor = null;
  if (card._tgSectionCache) card._tgSectionCache.thread = null;
  updateTelegramWidget(card, widgetId);

  const nextScroller = card.querySelector(".tg-messages");
  if (nextScroller) {
    nextScroller.scrollTop = nextScroller.scrollHeight;
    settleTelegramScroll(nextScroller);
  }

  const downBtn = card.querySelector(".tg-scroll-down-btn");
  if (downBtn) {
    downBtn.classList.remove("visible");
    const badge = downBtn.querySelector(".tg-scroll-down-badge");
    if (badge) {
      badge.textContent = "0";
      badge.style.display = "none";
    }
  }

  markTelegramChatRead(card, targetChatId, { force: true });
}

// Thread messages section: scroll restore, older-load, media, spoilers, attachments.
// Composer is intentionally not touched here so drafts/focus survive data refreshes.
// contentOnly: re-wire children after an in-place messages innerHTML replace without
// stacking scroll/click listeners on the scroller element itself.
function wireTelegramMessagesSection(card, widgetId, body, active, {
  previousScroll = null,
  prevChatId = null,
  prevThreadId = null,
  showAuth = false,
  restoreScroll = true,
  contentOnly = false,
} = {}) {
  const messageScroller = body.querySelector(".tg-messages");
  if (messageScroller) {
    const threadId = telegramForumThreadId(active?.id);
    messageScroller.dataset.chatId = active?.id || "";
    messageScroller.dataset.threadId = threadId || "";
    if (restoreScroll) {
      const anchor = telegramRuntime.pendingScrollAnchor;
      telegramRuntime.pendingScrollAnchor = null;
      const sameChat = prevChatId === (active?.id || "");
      const sameThread = sameChat && String(prevThreadId || "") === String(threadId || "");

      // Restore before paint. Geometry reads flush the inserted DOM's layout;
      // waiting two frames exposes the unscrolled history to the user.
      const performScrollRestore = () => {
        // Every programmatic scrollTop write emits a `scroll` event a frame later.
        // The guard prevents those from being treated as a "user scrolled near top"
        // gesture that would trigger an unwanted older-messages load.
        if (!messageScroller.isConnected) return;
        messageScroller._tgSuppressAutoLoad = true;
        if (anchor?.chatId === active?.id
            && String(anchor?.threadId || "") === String(threadId || "")
            && anchor?.messageId != null) {
          const item = messageScroller.querySelector(`[data-message-id="${CSS.escape(String(anchor.messageId))}"]`);
          if (item) {
            if (anchor.align === "center") {
              const itemOffset = tgMessageOffsetInScroller(item, messageScroller);
              const targetScroll = itemOffset - (messageScroller.clientHeight / 2) + (item.offsetHeight / 2);
              messageScroller.scrollTop = Math.max(0, targetScroll);
            } else {
              messageScroller.scrollTop = Math.max(0, tgMessageOffsetInScroller(item, messageScroller) - 12);
            }
          } else {
            messageScroller.scrollTop = messageScroller.scrollHeight;
          }
        } else if (sameThread && previousScroll) {
          restoreTelegramScroll(messageScroller, previousScroll);
        } else {
          messageScroller.scrollTop = messageScroller.scrollHeight;
        }
      };
      performScrollRestore();
      settleTelegramScroll(messageScroller);
    }

    if (!contentOnly) {
      // Remember the reading anchor before asynchronous media changes height.
      // A single delegated listener per scroller also handles later row patches.
      messageScroller.addEventListener('scroll', () => {
        messageScroller._tgReadingPosition = captureTelegramScroll(messageScroller);
        const scrollBtn = body.querySelector(".tg-scroll-down-btn");
        if (scrollBtn) {
          const distanceFromBottom = messageScroller.scrollHeight - messageScroller.scrollTop - messageScroller.clientHeight;
          if (telegramRuntime.detachedState) {
            scrollBtn.classList.add("visible");
          } else {
            scrollBtn.classList.toggle("visible", distanceFromBottom > 250);
          }
        }
      }, { passive: true });
      const preserveMediaPosition = () => {
        if (!messageScroller.isConnected || !messageScroller._tgReadingPosition) return;
        restoreTelegramScroll(messageScroller, messageScroller._tgReadingPosition);
        settleTelegramScroll(messageScroller);
      };
      messageScroller.addEventListener('load', preserveMediaPosition, true);
      messageScroller.addEventListener('loadedmetadata', preserveMediaPosition, true);
      messageScroller.addEventListener("scroll", () => {
        if (messageScroller._tgSuppressAutoLoad) return;
        const chatId = messageScroller.dataset.chatId;
        if (messageScroller.scrollTop > 100 || !chatId) return;
        loadTelegramOlderMessages(card, widgetId, chatId, messageScroller.dataset.threadId || null);
      });

      // External http(s) message links: open in system browser (Tauri does not honor target=_blank).
      messageScroller.addEventListener("click", (e) => {
        const anchor = e.target.closest?.("a[data-external-url]");
        if (!anchor || !messageScroller.contains(anchor)) return;
        e.preventDefault();
        e.stopPropagation();
        const url = anchor.getAttribute("data-external-url");
        if (url) invoke("plugin:opener|open_url", { url: url }).catch(() => false);
      });
    }

    body.querySelector(".tg-load-earlier")?.addEventListener("click", () => {
      const chatId = messageScroller?.dataset.chatId || active?.id;
      if (!chatId) return;
      loadTelegramOlderMessages(card, widgetId, chatId);
    });
  }
  const downBtn = body.querySelector(".tg-scroll-down-btn");
  if (downBtn && !downBtn._tgWired) {
    downBtn._tgWired = true;
    downBtn.addEventListener("click", (e) => {
      e.stopPropagation();
      restoreTelegramLiveTail(card, widgetId, active?.id);
    });
  }
  if (downBtn) {
    if (telegramRuntime.detachedState) {
      downBtn.classList.add("visible");
      const badge = downBtn.querySelector(".tg-scroll-down-badge");
      if (badge) {
        const count = telegramRuntime.detachedState.pendingTailMessages?.length || 0;
        badge.textContent = count > 99 ? "99+" : String(count);
        badge.style.display = count > 0 ? "inline-block" : "none";
      }
    } else if (messageScroller) {
      const distanceFromBottom = messageScroller.scrollHeight - messageScroller.scrollTop - messageScroller.clientHeight;
      downBtn.classList.toggle("visible", distanceFromBottom > 250);
    }
  }
  if (messageScroller) {
    const syncVisibleMedia = () => syncTelegramVisibleMedia(card, widgetId, messageScroller);
    if (!contentOnly) {
      messageScroller.addEventListener("scroll", () => {
        if (messageScroller._tgMediaRaf) return;
        messageScroller._tgMediaRaf = requestAnimationFrame(() => {
          messageScroller._tgMediaRaf = null;
          syncVisibleMedia();
        });
      }, { passive: true });
    }
    requestAnimationFrame(() => requestAnimationFrame(syncVisibleMedia));
  }
  body.querySelectorAll(".tg-message").forEach((row) => {
    wireTelegramMessageRow(card, widgetId, row, active);
  });

  // Inline keyboard button clicks
  body.querySelectorAll(".tg-inline-btn").forEach((btn) => {
    btn.addEventListener("click", async (e) => {
      e.stopPropagation();
      const btnType = btn.dataset.btnType;
      const url = btn.dataset.url;
      const data = btn.dataset.data;
      const chatId = btn.dataset.chatId;
      const msgId = btn.dataset.messageId;
      if (btnType === "inlineKeyboardButtonTypeUrl" && url) {
        invoke("plugin:opener|open_url", { url: url }).catch(() => false);
      } else if (btnType === "inlineKeyboardButtonTypeCallback" && data) {
        btn.classList.add("loading");
        try {
          await invoke("telegram_send_callback_query", { request: { chatId, messageId: msgId, data } });
        } catch (err) {
          logFrontend("callback query error: " + err);
        } finally {
          btn.classList.remove("loading");
        }
      }
    });
  });

  // Reply preview click -> scroll to original message (delegated so dynamically appended messages work)
  if (messageScroller && !messageScroller._replyWired) {
    messageScroller._replyWired = true;
    messageScroller.addEventListener("click", (e) => {
      const reply = e.target.closest(".tg-msg-reply");
      if (!reply) return;
      e.stopPropagation();
      const replyId = reply.dataset.replyId;
      if (!replyId) return;
      const chatId = active?.id || messageScroller.dataset.chatId;
      jumpToTelegramMessage(card, widgetId, chatId, replyId, reply);
    });
  }

  // Reactions click & pointer interaction (delegated so all messages & albums work)
  if (messageScroller && !messageScroller._rxWired) {
    messageScroller._rxWired = true;
    messageScroller.addEventListener("click", async (e) => {
      const pill = e.target.closest(".tg-rx-pill");
      if (!pill) return;
      e.stopPropagation();
      e.preventDefault();

      if (pill.dataset.pending === "true") return;
      pill.dataset.pending = "true";

      const chatId = pill.dataset.rxChatId || pill.dataset.chatId || messageScroller.dataset.chatId;
      const messageId = pill.dataset.messageId;
      if (!chatId || !messageId) {
        delete pill.dataset.pending;
        return;
      }

      const emoji = pill.dataset.emoji || null;
      const customEmojiId = pill.dataset.customEmojiId || null;
      const reactionType = pill.dataset.reactionType || (customEmojiId ? "customEmoji" : "emoji");
      const isChosen = pill.classList.contains("active");

      // Optimistic UI update
      const countEl = pill.querySelector(".tg-rx-count");
      const currentCount = parseInt(countEl?.textContent || "0", 10);
      if (isChosen) {
        pill.classList.remove("active");
        if (countEl) countEl.textContent = String(Math.max(0, currentCount - 1));
      } else {
        pill.classList.add("active");
        if (countEl) countEl.textContent = String(currentCount + 1);
      }

      try {
        await invoke("telegram_set_reaction", {
          request: {
            chatId,
            messageId,
            reactionType,
            emoji: emoji || undefined,
            customEmojiId: customEmojiId || undefined,
            remove: isChosen,
          },
        });
      } catch (err) {
        logFrontend("telegram_set_reaction error: " + err);
        // Rollback optimistic state
        if (isChosen) {
          pill.classList.add("active");
          if (countEl) countEl.textContent = String(currentCount);
        } else {
          pill.classList.remove("active");
          if (countEl) countEl.textContent = String(currentCount);
        }
      } finally {
        setTimeout(() => {
          delete pill.dataset.pending;
        }, 250);
      }
    });

    messageScroller.addEventListener("pointerdown", (e) => {
      if (e.target.closest(".tg-rx-pill")) {
        e.stopPropagation();
      }
    });
    messageScroller.addEventListener("mousedown", (e) => {
      if (e.target.closest(".tg-rx-pill")) {
        e.stopPropagation();
      }
    });
  }

  // Delegated media lightbox & pending download click handler
  if (messageScroller && !messageScroller._lightboxWired) {
    messageScroller._lightboxWired = true;
    messageScroller.addEventListener("click", (e) => {
      if (e.button !== 0) return;
      // Do not trigger if text is being selected
      const sel = window.getSelection ? window.getSelection() : null;
      if (sel && !sel.isCollapsed && sel.toString().trim().length > 0) return;

      const img = e.target.closest("img.tg-media:not(.tg-media-thumb)");
      if (img && messageScroller.contains(img)) {
        // Ignore previews inside videos, notes, or unrevealed spoilers
        if (img.closest(".tg-video-wrap, .tg-video-note-wrap, .tg-spoiler:not(.revealed)")) return;
        const src = img.currentSrc || img.src || img.getAttribute("src");
        if (src) {
          e.stopPropagation();
          if (document.querySelector(".tg-lightbox-overlay")) return;
          openTelegramLightbox(src, img);
          return;
        }
      }

      const pending = e.target.closest(".tg-media-pending:not(.tg-video-wrap):not(.tg-video-note-wrap)");
      if (pending && messageScroller.contains(pending)) {
        const fileId = pending.dataset.fileId;
        const msgId = pending.dataset.messageId;
        const chatId = pending.dataset.chatId || messageScroller.dataset.chatId;
        if (fileId && chatId) {
          e.stopPropagation();
          pending.classList.add("downloading");
          handleTelegramAttachmentDownload(card, widgetId, chatId, msgId, fileId, false);
        }
      }
    });
  }

  // Forum topics back button
  const forumBar = body.querySelector(".tg-forum-topics-bar");
  if (forumBar) {
    const backBtn = forumBar.querySelector("[data-action='back-to-topics']");
    if (backBtn) {
      backBtn.addEventListener("click", (e) => {
        e.stopPropagation();
        setTelegramForumThread(active?.id, null);
        telegramRuntime.pendingScrollAnchor = null;
        if (messageScroller) messageScroller._tgSuppressAutoLoad = false;
        renderTelegramWidget(card, widgetId);
      });
    }
  }

  // Pinned message bar click -> scroll to pinned message or unpin
  const pinnedBar = body.querySelector(".tg-pinned-bar");
  if (pinnedBar) {
    wireTelegramPinnedBar(pinnedBar, card, widgetId, active, messageScroller);
  }

  // Mark active chat read when the thread (not auth) is visible and messages are loaded.
  if (!showAuth && active?.id && telegramRuntime.messagesLoaded.has(telegramMessageCacheKey(active.id))) {
    markTelegramChatRead(card, active.id);
  }
}

function kickTelegramAvatarDownloads(card, widgetId, chats) {
  const candidates = card.classList.contains("expanded")
    ? telegramRenderedChats(chats)
    : chats.slice(0, TELEGRAM_RAIL_CHAT_LIMIT);
  const activeId = config.positions?.[widgetId]?.activeChatId;
  const active = chats.find((chat) => chat.id === activeId);
  if (active && !candidates.some((chat) => chat.id === active.id)) candidates.push(active);
  candidates.forEach((chat) => {
    if (chat.photoFileId && !chat.photoPath) {
      handleTelegramAvatarDownload(card, widgetId, chat.id, chat.photoFileId);
    }
  });
}

// Synchronize composer bars (reply, edit, pending file from explorer) and placeholder without destroying input focus
function updateTelegramComposerBars(card, widgetId, body, active) {
  if (!body) return;
  const composer = body.querySelector(".tg-composer");
  if (!composer) return;
  const chatId = active?.id;
  const replyMsg = chatId && telegramRuntime.replyMessage ? telegramRuntime.replyMessage.get(chatId) : null;
  const editMsg = chatId && telegramRuntime.editMessage ? telegramRuntime.editMessage.get(chatId) : null;
  const pendingFile = telegramRuntime.pendingExternalFile;

  // 1. Reply bar
  let replyEl = composer.querySelector(".tg-reply-bar:not(.tg-edit-bar)");
  if (replyMsg) {
    if (!replyEl) {
      replyEl = document.createElement("div");
      replyEl.className = "tg-reply-bar";
      const row = composer.querySelector(".tg-composer-row");
      if (row) composer.insertBefore(replyEl, row);
      else composer.appendChild(replyEl);
    }
    replyEl.innerHTML = `
      <div class="tg-reply-bar-content">
        <span class="tg-reply-bar-author">${escapeHtml(replyMsg.from || "Message")}</span>
        <span class="tg-reply-bar-text">${escapeHtml(replyMsg.text || "...")}</span>
      </div>
      <button type="button" class="tg-reply-bar-close" aria-label="Cancel reply">×</button>
    `;
  } else if (replyEl) {
    replyEl.remove();
  }

  // 2. Edit bar
  let editEl = composer.querySelector(".tg-edit-bar");
  if (editMsg) {
    if (!editEl) {
      editEl = document.createElement("div");
      editEl.className = "tg-reply-bar tg-edit-bar";
      const row = composer.querySelector(".tg-composer-row");
      if (row) composer.insertBefore(editEl, row);
      else composer.appendChild(editEl);
    }
    editEl.innerHTML = `
      <div class="tg-reply-bar-content">
        <span class="tg-reply-bar-author">Editing</span>
        <span class="tg-reply-bar-text">${escapeHtml(editMsg.text || "...")}</span>
      </div>
      <button type="button" class="tg-edit-bar-close" aria-label="Cancel edit">×</button>
    `;
  } else if (editEl) {
    editEl.remove();
  }

  // 3. Pending file bar
  let pendingEl = composer.querySelector(".tg-pending-file");
  if (pendingFile) {
    const pendingFileName = pendingFile.split(/[/\\]/).pop() || pendingFile;
    if (!pendingEl) {
      pendingEl = document.createElement("div");
      pendingEl.className = "tg-pending-file";
      pendingEl.dataset.path = pendingFile;
      pendingEl.innerHTML = `
        <span class="tg-pending-file-name" aria-label="${escapeHtml(pendingFile)}">${escapeHtml(pendingFileName)}</span>
        <button type="button" class="tg-pending-file-remove" aria-label="Remove attached file">×</button>
      `;
      const row = composer.querySelector(".tg-composer-row");
      if (row) composer.insertBefore(pendingEl, row);
      else composer.appendChild(pendingEl);
    } else if (pendingEl.dataset.path !== pendingFile) {
      pendingEl.dataset.path = pendingFile;
      const nameEl = pendingEl.querySelector(".tg-pending-file-name");
      if (nameEl) {
        nameEl.setAttribute("aria-label", pendingFile);
        nameEl.textContent = pendingFileName;
      }
    }
  } else if (pendingEl) {
    pendingEl.remove();
  }

  // 4. Placeholder
  const input = composer.querySelector(".tg-input");
  if (input) {
    const desiredPlaceholder = pendingFile ? "Add a caption..." : editMsg ? "Edit message..." : "Message";
    if (input.placeholder !== desiredPlaceholder) {
      input.placeholder = desiredPlaceholder;
    }
  }
}

// Lightweight in-place update for data-refresh paths. Never replaces the composer.
// Falls back to full rebuild when shell is missing or structural mode changed.
function updateTelegramWidget(card, widgetId) {
  if (!card?.isConnected) return;
  if (card._tgTransitioning || card._tgSurfaceTransitioning) {
    card._tgPendingUpdate = true;
    return;
  }
  const body = card.querySelector(".widget-body");
  if (!body?.querySelector(".tg-shell")) {
    renderTelegramWidget(card, widgetId);
    return;
  }

  const pos = config.positions[widgetId] || {};
  const chats = telegramVisibleChats();
  const activeId = pos.activeChatId || chats.find((chat) => chat.unread)?.id || chats[0]?.id;
  const active = chats.find((chat) => chat.id === activeId) || chats[0];
  pos.activeChatId = active?.id;
  const status = telegramRuntime.status;
  const showAuth = (pos.showSettings === true) || shouldShowTelegramAuth(status);
  const expanded = card.classList.contains("expanded");
  const hasPanel = !!body.querySelector(".tg-panel");
  if (expanded !== hasPanel) {
    renderTelegramWidget(card, widgetId);
    return;
  }

  // The collapsed dock only owns its short avatar rail. Avoid constructing or
  // diffing the hidden chat list/thread and do not wake media decoding.
  if (!expanded) {
    const railHtml = buildRailHtml(chats, active?.id);
    const cache = card._tgSectionCache || (card._tgSectionCache = { rail: null, list: null, thread: null });
    const railEl = body.querySelector(".tg-rail");
    if (railEl && railHtml !== cache.rail) {
      railEl.innerHTML = railHtml;
      cache.rail = railHtml;
      wireTelegramRailSection(card, widgetId, body);
    }
    kickTelegramAvatarDownloads(card, widgetId, chats);
    return;
  }

  const activeThreadId = telegramForumThreadId(active?.id);
  const activeMessageKey = telegramMessageCacheKey(active?.id, activeThreadId);
  if (active && !telegramRuntime.messages.has(activeMessageKey)) {
    cacheTelegramMessages(active.id, []);
  }

  const threadEl = body.querySelector(".tg-thread");
  const isAuthDom = !!threadEl?.classList.contains("auth");
  if (isAuthDom !== !!showAuth) {
    renderTelegramWidget(card, widgetId);
    return;
  }

  const messageScroller = body.querySelector(".tg-messages");
  if (!showAuth) {
    if (!messageScroller) {
      renderTelegramWidget(card, widgetId);
      return;
    }
    const domChatId = messageScroller.dataset.chatId || "";
    const domThreadId = messageScroller.dataset.threadId || "";
    if ((active?.id || "") !== domChatId || String(activeThreadId || "") !== domThreadId) {
      renderTelegramWidget(card, widgetId);
      return;
    }
  }

  const msgKey = telegramMessageCacheKey(active?.id, activeThreadId);
  const messages = telegramRuntime.messages.get(msgKey) || [];
  const railHtml = buildRailHtml(chats, active?.id);
  // Forum chats keep the collapsed chat-list + topics column layout. When a
  // topic is selected the thread shows that topic's messages; otherwise it
  // shows a hint. The thread pane never shrinks.
  const forumChatId = telegramRuntime.forumOpenChatId;
  const forumChat = forumChatId ? chats.find((c) => c.id === forumChatId) : null;
  const showForumTopics = !!forumChat;
  let listHtml = buildChatListHtml(chats, active?.id, telegramChatListState(chats));
  let topicsHtml = "";
  if (showForumTopics) {
    topicsHtml = buildForumTopicsListHtml(forumChat);
    if (!telegramRuntime.forumTopics.has(forumChat.id)) {
      loadForumTopics(forumChat.id);
    }
  }
  const threadHtml = showAuth ? "" : buildMessagesInnerHtml(active, messages);
  // Forum chat with no topic selected: never show the general chat in the
  // thread — show a hint instead (topics live in the topics column).
  const showTopicHint = !showAuth && showForumTopics && active?.id === forumChat?.id && !telegramForumThreadId(forumChat.id);
  const effectiveThreadHtml = showTopicHint ? '<div class="tg-state">Select a topic</div>' : threadHtml;
  const cache = card._tgSectionCache || (card._tgSectionCache = { rail: null, list: null, thread: null, topics: null, topicsChatId: null });

  const railEl = body.querySelector(".tg-rail");
  if (railEl && railHtml !== cache.rail) {
    railEl.innerHTML = railHtml;
    cache.rail = railHtml;
    wireTelegramRailSection(card, widgetId, body);
  }

  const listEl = body.querySelector(".tg-chat-list");
  if (listEl && listHtml !== cache.list) {
    const listScroll = captureTelegramListScroll(listEl);
    listEl.innerHTML = listHtml;
    cache.list = listHtml;
    wireTelegramListSection(card, widgetId, body);
    restoreTelegramListScroll(listEl, listScroll);
  }

  // Render topics into their own column (layout switch handled by CSS class).
  const topicsEl = body.querySelector(".tg-topics-list");
  const topicsChanged = topicsEl && (topicsHtml !== cache.topics || forumChat?.id !== cache.topicsChatId);
  if (topicsEl) {
    if (showForumTopics) {
      if (topicsChanged) {
        const previousTopicScroll = topicsEl.scrollTop;
        const focusedSearch = topicsEl.querySelector('.tg-topic-search');
        const hadSearchFocus = focusedSearch === document.activeElement;
        const selection = focusedSearch ? [focusedSearch.selectionStart, focusedSearch.selectionEnd] : null;
        topicsEl.innerHTML = topicsHtml;
        topicsEl.scrollTop = previousTopicScroll;
        cache.topics = topicsHtml;
        cache.topicsChatId = forumChat?.id || null;
        wireTelegramTopicsSection(card, widgetId, body, active);
        if (hadSearchFocus) {
          const nextSearch = topicsEl.querySelector('.tg-topic-search');
          nextSearch?.focus({ preventScroll: true });
          if (selection && nextSearch) {
            try { nextSearch.setSelectionRange(...selection); } catch {}
          }
        }
      }
    }
    // Chat list column collapses when topics are shown; both panes animate.
    body.querySelector(".tg-layout")?.classList.toggle("forum-topics", showForumTopics);
    syncTelegramTopicsVisibility(body, showForumTopics);
  }


  if (!showAuth && messageScroller && effectiveThreadHtml !== cache.thread) {
    const previousScroll = captureTelegramScroll(messageScroller);
    const prevChatId = messageScroller.dataset.chatId || null;
    const prevThreadId = messageScroller.dataset.threadId || null;
    messageScroller.innerHTML = effectiveThreadHtml;
    cache.thread = effectiveThreadHtml;
    wireTelegramMessagesSection(card, widgetId, body, active, {
      previousScroll,
      prevChatId,
      prevThreadId,
      showAuth: false,
      restoreScroll: true,
      contentOnly: true,
    });
  } else if (!showAuth && active?.id && telegramRuntime.messagesLoaded.has(telegramMessageCacheKey(active.id))) {
    // Messages section unchanged, but still honor mark-read (same as full rebuild).
    markTelegramChatRead(card, active.id);
  }

  updateTelegramComposerBars(card, widgetId, body, active);
  kickTelegramAvatarDownloads(card, widgetId, chats);
}

// ─── Telegram context menus (right-click, like desktop Telegram) ───

let tgContextMenuEl = null;

function closeTelegramContextMenu() {
  if (tgContextMenuEl) {
    const el = tgContextMenuEl;
    tgContextMenuEl = null;
    if (surfaceManager.isOpen(el)) {
      surfaceManager.closeSurface(el);
    } else {
      el.remove();
    }
  }
}

async function openTelegramLightbox(src, originEl = null) {
  if (!src) return;
  const overlay = document.createElement("div");
  overlay.className = "tg-lightbox-overlay";
  overlay.innerHTML = `
    <div class="tg-lightbox-content">
      <img src="${escapeHtml(src)}" alt="" class="tg-lightbox-img" draggable="false">
    </div>
    <button type="button" class="tg-lightbox-close" aria-label="Close">
      <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
        <line x1="18" y1="6" x2="6" y2="18"></line>
        <line x1="6" y1="6" x2="18" y2="18"></line>
      </svg>
    </button>
  `;
  const content = overlay.querySelector(".tg-lightbox-content");
  const imgEl = overlay.querySelector(".tg-lightbox-img");
  const closeBtn = overlay.querySelector(".tg-lightbox-close");

  const close = () => {
    if (content && surfaceManager.isOpen(content)) {
      surfaceManager.closeSurface(content);
    } else {
      overlay.remove();
    }
  };

  closeBtn?.addEventListener("click", (e) => {
    e.stopPropagation();
    close();
  });

  // Ensure image is loaded/decoded before measuring geometry in surfaceManager to prevent CLS
  if (imgEl) {
    try {
      if (!imgEl.complete) {
        await imgEl.decode();
      }
    } catch (_) {}
  }

  surfaceManager.getPortal().appendChild(overlay);

  surfaceManager.openSurface(content, {
    backdrop: overlay,
    backdropDisplay: "flex",
    origin: (originEl instanceof HTMLElement && originEl.isConnected) ? originEl : null,
    originRadius: "16px",
    elevation: "var(--z-surface-modal, 3000)",
    closeOnOutsideClick: true,
    closeOnEscape: true,
    onOpen: () => {
      overlay.classList.add("visible");
    },
    onClosed: () => {
      if (imgEl) imgEl.src = "";
      overlay.remove();
    },
  });
}

function openTelegramForwardPicker(card, widgetId, fromChatId, messageId, previewText) {
  closeTelegramContextMenu();
  const chats = telegramVisibleChats();
  if (!chats.length) return;
  const overlay = document.createElement("div");
  overlay.className = "tg-forward-modal-overlay";
  overlay.dataset.surfaceTint = "telegram"; // portaled: tint is not inherited from the card
  overlay.innerHTML = `
    <div class="tg-forward-modal">
      <div class="tg-forward-header">
        <strong>Forward message</strong>
        <small>${escapeHtml(previewText.slice(0, 60))}</small>
      </div>
      <div class="tg-forward-list">
        ${chats.map((c) => `
          <button type="button" class="tg-forward-item" data-chat-id="${escapeHtml(c.id)}">
            <span class="tg-forward-avatar">${telegramAvatarInner(c)}</span>
            <span class="tg-forward-title">${escapeHtml(c.title)}</span>
          </button>
        `).join("")}
      </div>
      <button type="button" class="tg-forward-close">Cancel</button>
    </div>
  `;
  const modal = overlay.querySelector(".tg-forward-modal");
  const close = () => {
    if (modal && surfaceManager.isOpen(modal)) {
      surfaceManager.closeSurface(modal);
    } else {
      overlay.remove();
    }
  };
  overlay.querySelector(".tg-forward-close")?.addEventListener("click", close);
  overlay.addEventListener("click", (e) => {
    if (e.target === overlay) close();
  });
  overlay.querySelectorAll(".tg-forward-item").forEach((btn) => {
    btn.addEventListener("click", async (e) => {
      e.stopPropagation();
      const toChatId = btn.dataset.chatId;
      if (!toChatId) return;
      close();
      try {
        await invoke("telegram_forward_messages", { request: { toChatId: toChatId, fromChatId: fromChatId, messageIds: [messageId] } });
        logFrontend(`Forwarded message ${messageId} to chat ${toChatId}`);
      } catch (err) {
        logFrontend("forward error: " + err);
      }
    });
  });
  surfaceManager.getPortal().appendChild(overlay);

  surfaceManager.openSurface(modal, {
    backdrop: overlay,
    originRadius: "16px",
    elevation: "var(--z-surface-modal, 3000)",
    onOpen: () => {
      overlay.classList.add("visible");
    },
    onClosed: () => {
      overlay.remove();
    },
  });
}

function openTelegramSendFilePicker(card, widgetId, filePath) {
  closeTelegramContextMenu();
  const chats = telegramVisibleChats();
  const fileName = filePath.split(/[/\\]/).pop() || filePath;
  if (!chats.length) {
    telegramRuntime.pendingExternalFile = filePath;
    const pos = config.positions[widgetId] || {};
    if (pos.activeChatId) {
      telegramSetActiveChat(card, widgetId, pos.activeChatId);
    } else {
      renderTelegramWidget(card, widgetId);
    }
    return;
  }
  document.querySelectorAll(".tg-forward-modal-overlay").forEach((el) => el.remove());
  const overlay = document.createElement("div");
  overlay.className = "tg-forward-modal-overlay";
  overlay.dataset.surfaceTint = "telegram"; // portaled: tint is not inherited from the card
  overlay.innerHTML = `
    <div class="tg-forward-modal">
      <div class="tg-forward-header">
        <strong>Send file to...</strong>
        <small>${escapeHtml(fileName)}</small>
      </div>
      <div class="tg-forward-list">
        ${chats.map((c) => `
          <button type="button" class="tg-forward-item" data-chat-id="${escapeHtml(c.id)}">
            <span class="tg-forward-avatar">${telegramAvatarInner(c)}</span>
            <span class="tg-forward-title">${escapeHtml(c.title)}</span>
          </button>
        `).join("")}
      </div>
      <button type="button" class="tg-forward-close">Cancel</button>
    </div>
  `;
  const modal = overlay.querySelector(".tg-forward-modal");
  const close = () => {
    if (modal && surfaceManager.isOpen(modal)) {
      surfaceManager.closeSurface(modal);
    } else {
      overlay.remove();
    }
  };
  overlay.querySelector(".tg-forward-close")?.addEventListener("click", close);
  overlay.addEventListener("click", (e) => {
    if (e.target === overlay) close();
  });
  overlay.querySelectorAll(".tg-forward-item").forEach((btn) => {
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      const toChatId = btn.dataset.chatId;
      if (!toChatId) return;
      close();
      telegramRuntime.pendingExternalFile = filePath;
      telegramSetActiveChat(card, widgetId, toChatId);
    });
  });
  surfaceManager.getPortal().appendChild(overlay);

  surfaceManager.openSurface(modal, {
    backdrop: overlay,
    originRadius: "16px",
    elevation: "var(--z-surface-modal, 3000)",
    onOpen: () => {
      overlay.classList.add("visible");
    },
    onClosed: () => {
      overlay.remove();
    },
  });
}

function openTelegramContextMenu(x, y, items, options = {}) {
  closeTelegramContextMenu();
  const entries = (items || []).filter(Boolean);
  if (!entries.length && !options?.reactions) return;
  const menu = document.createElement("div");
  menu.className = "tg-context-menu";
  menu.dataset.surfaceTint = "telegram"; // portaled: tint is not inherited from the card

  if (options?.reactions?.list?.length) {
    const rxBar = document.createElement("div");
    rxBar.className = "tg-cm-reactions";
    options.reactions.list.forEach((emoji) => {
      const rxBtn = document.createElement("button");
      rxBtn.type = "button";
      rxBtn.className = "tg-cm-rx-btn";
      rxBtn.textContent = emoji;
      rxBtn.addEventListener("click", (e) => {
        e.stopPropagation();
        closeTelegramContextMenu();
        options.reactions.onSelect?.(emoji);
      });
      rxBar.appendChild(rxBtn);
    });
    menu.appendChild(rxBar);
  }

  entries.forEach((item) => {
    const btn = document.createElement("button");
    btn.type = "button";
    btn.textContent = item.label;
    if (item.danger) btn.className = "danger";
    btn.disabled = !!item.disabled;
    btn.addEventListener("click", (e) => {
      e.stopPropagation();
      closeTelegramContextMenu();
      item.onClick?.();
    });
    menu.appendChild(btn);
  });
  surfaceManager.getPortal().appendChild(menu);
  // Clamp inside the viewport (menu is position: fixed, coords are client).
  const rect = menu.getBoundingClientRect();
  menu.style.left = Math.max(4, Math.min(x, window.innerWidth - rect.width - 4)) + "px";
  menu.style.top = Math.max(4, Math.min(y, window.innerHeight - rect.height - 4)) + "px";
  tgContextMenuEl = menu;

  surfaceManager.openSurface(menu, {
    origin: { left: x, top: y, width: 4, height: 4 },
    originRadius: "8px",
    elevation: "var(--z-surface-menu, 4000)",
    onClosed: () => {
      menu.remove();
      if (tgContextMenuEl === menu) tgContextMenuEl = null;
    },
  });
}

// Global input focus fix for WebView2: when the user clicks on an input field,
// the WebView2 window might not have OS focus, so the input field doesn't receive
// focus. This handler focuses the input field when it's clicked.
document.addEventListener("pointerdown", (e) => {
  const input = e.target.closest("input, textarea, [contenteditable]");
  if (input && document.activeElement !== input) {
    try { input.focus({ preventScroll: true }); } catch { input.focus(); }
  }
});
window.addEventListener("blur", closeTelegramContextMenu);

async function copyTelegramText(text) {
  if (!text) return;
  // The wallpaper window never has OS focus, so the JS clipboard API is
  // unreliable; the backend command writes CF_UNICODETEXT directly.
  try {
    await invoke("set_clipboard_text", { text: text });
    return;
  } catch (_) { /* fall through to the web API */ }
  try {
    await navigator.clipboard.writeText(text);
  } catch (_) { /* clipboard unavailable */ }
}

function openTelegramMessageMenu(card, widgetId, body, msgEl, x, y) {
  const selection = window.getSelection();
  const selectedText = selection && !selection.isCollapsed && msgEl.contains(selection.anchorNode)
    ? selection.toString().trim()
    : "";
  const fullText = (msgEl.querySelector(".tg-text")?.innerText || "").trim();
  const msgId = msgEl.dataset.messageId || "";
  const chatId = config.positions?.[widgetId]?.activeChatId || body.querySelector(".tg-messages")?.dataset.chatId || "";
  const currentList = telegramRuntime.messages.get(telegramMessageCacheKey(chatId, body.querySelector(".tg-messages")?.dataset.threadId || null)) || [];
  const messageObj = currentList.find((m) => String(m.id) === String(msgId));
  const isMine = messageObj?.from === "me" || msgEl.classList.contains("out");

  const reactions = ["👍", "❤️", "🔥", "😂", "😮", "😢", "🎉"];
  const items = [];

  // Reply action
  items.push({
    label: "Reply",
    onClick: () => {
      if (!chatId) return;
      if (!telegramRuntime.replyMessage) telegramRuntime.replyMessage = new Map();
      telegramRuntime.replyMessage.set(chatId, {
        id: msgId,
        text: fullText || messageObj?.attachment || "Message",
        from: messageObj?.from || "User",
      });
      updateTelegramWidget(card, widgetId);
      const composer = body.querySelector(".tg-input");
      composer?.focus();
    },
  });

  // Edit action (only for user's own text messages)
  if (isMine && fullText) {
    items.push({
      label: "Edit",
      onClick: () => {
        if (!chatId) return;
        if (!telegramRuntime.editMessage) telegramRuntime.editMessage = new Map();
        telegramRuntime.editMessage.set(chatId, { id: msgId, text: fullText });
        updateTelegramWidget(card, widgetId);
        const composer = body.querySelector(".tg-input");
        if (composer) {
          composer.value = fullText;
          composer.focus();
          composer.dispatchEvent(new Event("input", { bubbles: true }));
        }
      },
    });
  }

  // Copy actions
  if (selectedText) {
    items.push({ label: "Copy selection", onClick: () => copyTelegramText(selectedText) });
  }
  if (fullText && fullText !== selectedText) {
    items.push({ label: "Copy text", onClick: () => copyTelegramText(fullText) });
  }

  // Pin / Unpin action
  const currentChat = telegramRuntime.chats?.find((c) => c.id === chatId);
  const threadId = telegramForumThreadId(chatId);
  const pinKey = threadId ? `${chatId}:${threadId}` : String(chatId);
  const currentPinnedId = currentChat?.pinnedMessageId || telegramRuntime.pinnedMessages?.get(pinKey);
  const isCurrentlyPinned = (currentPinnedId && String(currentPinnedId) === String(msgId))
    || messageObj?.isPinned === true;

  if (isCurrentlyPinned) {
    items.push({
      label: "Unpin message",
      onClick: async () => {
        if (!chatId || !msgId) return;
        await handleUnpinTelegramMessage(card, widgetId, chatId, msgId);
        updateTelegramWidget(card, widgetId);
      },
    });
  } else {
    items.push({
      label: "Pin message",
      onClick: async () => {
        if (!chatId || !msgId) return;
        try {
          await invoke("telegram_pin_message", { request: { chatId: chatId, messageId: msgId } });
          if (!telegramRuntime.pinnedMessages) telegramRuntime.pinnedMessages = new Map();
          telegramRuntime.pinnedMessages.set(pinKey, String(msgId));
          if (currentChat) currentChat.pinnedMessageId = String(msgId);
          if (messageObj) {
            messageObj.isPinned = true;
            if (!telegramRuntime.pinnedMessageSnippet) telegramRuntime.pinnedMessageSnippet = new Map();
            telegramRuntime.pinnedMessageSnippet.set(pinKey, messageObj);
          }
          await ensureTelegramChatPinnedMessage(chatId, threadId, true);
          updateTelegramWidget(card, widgetId);
        } catch (err) {
          logFrontend("pin message error: " + err);
        }
      },
    });
  }

  // Forward action
  items.push({
    label: "Forward",
    onClick: () => {
      openTelegramForwardPicker(card, widgetId, chatId, msgId, fullText || messageObj?.attachment || "Message");
    },
  });

  // Delete action
  items.push({
    label: "Delete message",
    danger: true,
    onClick: async () => {
      if (!chatId || !msgId) return;
      try {
        await invoke("telegram_delete_messages", { request: { chatId, messageIds: [msgId], revoke: true } });
        removeTelegramRuntimeMessages(chatId, [msgId]);
        removeTelegramMessageDom(card, chatId, [msgId]);
      } catch (err) {
        logFrontend("delete message error: " + err);
      }
    },
  });

  openTelegramContextMenu(x, y, items, {
    reactions: {
      list: reactions,
      onSelect: async (emoji) => {
        if (!chatId || !msgId) return;
        try {
          await invoke("telegram_set_reaction", { request: { chatId, messageId: msgId, emoji } });
        } catch (err) {
          logFrontend("set reaction error: " + err);
        }
      },
    },
  });
}

function openTelegramChatMenu(card, widgetId, chatId, x, y) {
  if (!chatId) return;
  const chat = [...(telegramRuntime.chats || []), ...(telegramRuntime.archivedChats || [])]
    .find((c) => String(c.id) === String(chatId));
  const archived = telegramRuntime.chatListView === "archive";
  openTelegramContextMenu(x, y, [
    {
      label: "Open",
      onClick: () => {
        telegramSetActiveChat(card, widgetId, chatId);
        if (!card.classList.contains("expanded")) toggleTelegramWidget(card, widgetId);
      },
    },
    {
      label: archived ? "Unarchive" : "Archive",
      onClick: () => setTelegramChatArchived(card, widgetId, chatId, !archived),
    },
    {
      label: "Mark as read",
      onClick: () => {
        try {
          telegramRuntime.lastMarkedRead.delete(chatId);
          const p = invoke("telegram_mark_read", { chatId: chatId });
          if (p && typeof p.then === "function") {
            p.then(() => {
              if (chat) {
                chat.unread = 0;
                const card = getTelegramCard();
                if (card) patchTelegramChatRowDom(card, chat);
              }
            }).catch(() => {});
          }
        } catch (_) {}
      },
    },
    {
      label: "Clear history",
      danger: true,
      onClick: async () => {
        try {
          await invoke("telegram_delete_chat_history", { request: { chatId: chatId, removeFromChatList: false, revoke: true } });
          cacheTelegramMessages(chatId, []);
          updateTelegramWidget(card, widgetId);
        } catch (err) {
          logFrontend("clear history error: " + err);
        }
      },
    },
    {
      label: "Delete chat",
      danger: true,
      onClick: async () => {
        try {
          await invoke("telegram_delete_chat_history", { request: { chatId: chatId, removeFromChatList: true, revoke: true } });
          if (Array.isArray(telegramRuntime.chats)) {
            telegramRuntime.chats = telegramRuntime.chats.filter((c) => c.id !== chatId);
          }
          cacheTelegramMessages(chatId, []);
          updateTelegramWidget(card, widgetId);
        } catch (err) {
          logFrontend("delete chat error: " + err);
        }
      },
    },
  ]);
}

// One delegated contextmenu listener per widget body; survives innerHTML
// rebuilds (guarded against re-attachment on every render).
function wireTelegramContextMenus(card, widgetId, body) {
  if (body._tgContextWired) return;
  body._tgContextWired = true;
  body.addEventListener("contextmenu", (e) => {
    const msgEl = e.target.closest?.(".tg-message");
    if (msgEl && body.contains(msgEl)) {
      e.preventDefault();
      e.stopPropagation();
      openTelegramMessageMenu(card, widgetId, body, msgEl, e.clientX, e.clientY);
      return;
    }
    // Chat rows in the list, rail avatars, in-message chat references.
    const chatEl = e.target.closest?.("[data-chat-id]");
    if (chatEl && body.contains(chatEl) && chatEl.dataset.chatId) {
      e.preventDefault();
      e.stopPropagation();
      openTelegramChatMenu(card, widgetId, chatEl.dataset.chatId, e.clientX, e.clientY);
    }
  });
}

// ─── Composer @-mention autocomplete ───
// Single telegram widget → one popup. `entities` holds pending mentionName
// entities for picked users who have no public @username (a plain text
// mention cannot tag them); validated against the final text on send.
const tgMention = {
  open: false,
  items: [],
  index: 0,
  chatId: "",
  tokenStart: 0,
  seq: 0,
  debounce: null,
  entities: [],
};

function tgMentionContext(input) {
  const caret = input.selectionStart ?? 0;
  if (caret !== input.selectionEnd) return null;
  const before = input.value.slice(0, caret);
  const match = /(^|\s)@([^\s@]{0,32})$/.exec(before);
  if (!match) return null;
  return { start: caret - match[2].length - 1, query: match[2] };
}

function closeTgMentionPop(body) {
  tgMention.open = false;
  tgMention.items = [];
  tgMention.index = 0;
  const pop = body?.querySelector(".tg-mention-pop");
  if (pop) pop.innerHTML = "";
}

function renderTgMentionPop(body) {
  const pop = body?.querySelector(".tg-mention-pop");
  if (!pop) return;
  if (!tgMention.open || !tgMention.items.length) {
    pop.innerHTML = "";
    return;
  }
  pop.innerHTML = tgMention.items.map((member, i) => `
    <button type="button" class="tg-mention-item ${i === tgMention.index ? "active" : ""}" data-mention-index="${i}">
      <span class="tg-mention-name">${escapeHtml(member.name)}</span>
      ${member.username ? `<span class="tg-mention-user">@${escapeHtml(member.username)}</span>` : ""}
    </button>
  `).join("");
}

function scheduleTgMentionFetch(body, chat) {
  const input = body?.querySelector(".tg-input");
  const ctx = input ? tgMentionContext(input) : null;
  // Mentions only make sense where members exist; private/secret chats and
  // the preview mode (no backend command) never open the popup.
  const mentionable = chat && chat.kind !== "private" && chat.kind !== "secret";
  if (!ctx || !mentionable) {
    closeTgMentionPop(body);
    return;
  }
  tgMention.chatId = chat.id;
  tgMention.tokenStart = ctx.start;
  const seq = ++tgMention.seq;
  clearTimeout(tgMention.debounce);
  tgMention.debounce = setTimeout(async () => {
    let members = [];
    try {
      members = await invoke("telegram_chat_members", { chatId: chat.id, query: ctx.query }) || [];
    } catch (_) { members = []; }
    if (seq !== tgMention.seq) return;
    // Caret context may have moved on while the request was in flight.
    const liveInput = body.querySelector(".tg-input");
    const liveCtx = liveInput ? tgMentionContext(liveInput) : null;
    if (!liveCtx) {
      closeTgMentionPop(body);
      return;
    }
    tgMention.items = members.filter((m) => m && (m.name || m.username));
    tgMention.index = 0;
    tgMention.open = tgMention.items.length > 0;
    renderTgMentionPop(body);
  }, 200);
}

function pickTgMention(body) {
  const input = body?.querySelector(".tg-input");
  const item = tgMention.items[tgMention.index];
  if (!input || !item) return;
  const ctx = tgMentionContext(input) || { start: tgMention.tokenStart };
  const caret = input.selectionStart ?? ctx.start;
  const before = input.value.slice(0, ctx.start);
  const after = input.value.slice(caret);
  const insert = item.username ? `@${item.username}` : item.name;
  input.value = `${before}${insert} ${after}`;
  const newCaret = before.length + insert.length + 1;
  try { input.setSelectionRange(newCaret, newCaret); } catch (_) { /* stale range */ }
  if (!item.username && item.userId) {
    // No public username → a real mention needs a mentionName entity over
    // the inserted display-name text (validated again right before send).
    tgMention.entities.push({
      offset: ctx.start,
      length: insert.length,
      text: insert,
      userId: String(item.userId),
    });
  }
  closeTgMentionPop(body);
  input.focus();
  // Run the existing input handler (draft save + auto-grow + mention rescan).
  input.dispatchEvent(new Event("input", { bubbles: true }));
}

window.renderTelegramWidget = renderTelegramWidget;

// Shared by a full render and a morph handoff; queries stay inside one thread.
function wireTelegramComposer(card, widgetId, body, active, capturedComposer = {}) {
  if (!body || body._tgComposerWired) return;
  body._tgComposerWired = true;
  const generationAtBind = telegramRuntime.accountGeneration;
  const boundThreadId = telegramForumThreadId(active?.id);
  const isCurrent = () => body.isConnected && !card._tgTransitioning
    && generationAtBind === telegramRuntime.accountGeneration
    && config.positions[widgetId]?.activeChatId === active?.id
    && telegramForumThreadId(active?.id) === boundThreadId;
  const sendCurrentMessage = async () => {
    // In-flight guard: rapid Enter + click must not produce two TDLib sendMessage calls.
    if (!isCurrent() || telegramRuntime.sending) return;
    const input = body.querySelector(".tg-input");
    const pendingFile = telegramRuntime.pendingExternalFile;
    const text = input?.value?.replace(/\n+$/, "").trim();
    if (!text && !pendingFile) return;
    if (!isCurrent() || !active?.id) return;
    const chatId = active.id;
    const threadId = boundThreadId;
    const generation = telegramRuntime.accountGeneration;
    const sendBtn = body.querySelector(".tg-send");
    telegramRuntime.sending = true;
    if (sendBtn) sendBtn.classList.add("sending");
    if (input) {
      input.value = "";
      input.removeAttribute("aria-label");
    }
    telegramRuntime.drafts.delete(chatId);

    // Edit message mode
    const editing = telegramRuntime.editMessage?.get(chatId);
    if (editing) {
      telegramRuntime.editMessage.delete(chatId);
      try {
        const edited = await invoke("telegram_edit_message", { request: { chatId: chatId, messageId: editing.id, text: text } });
        if (generation !== telegramRuntime.accountGeneration) return;
        if (edited) {
          replaceTelegramRuntimeMessage(chatId, editing.id, edited);
          replaceTelegramMessageDom(card, widgetId, chatId, editing.id, edited);
        }
      } catch (error) {
        logFrontend("edit message error: " + error);
      } finally {
        if (generation === telegramRuntime.accountGeneration) {
          telegramRuntime.sending = false;
          if (sendBtn) sendBtn.classList.remove("sending");
          updateTelegramWidget(card, widgetId);
        }
      }
      return;
    }

    // Reply message mode
    const replying = telegramRuntime.replyMessage?.get(chatId);
    if (replying) {
      telegramRuntime.replyMessage.delete(chatId);
    }
    const replyToId = replying?.id || null;

    let sent = null;
    // Messages in a selected forum topic are sent to that topic's thread.
    try {
      if (pendingFile) {
        sent = await invoke("telegram_send_attachment", { request: { chatId: chatId, path: pendingFile, caption: text || "", replyToMessageId: replyToId, threadId: threadId ? Number(threadId) || null : null } });
        if (generation === telegramRuntime.accountGeneration && telegramRuntime.pendingExternalFile === pendingFile) {
          telegramRuntime.pendingExternalFile = null;
        }
      } else {
        const mentionEntities = tgMention.entities
          .filter((e) => e.userId && text.slice(e.offset, e.offset + e.length) === e.text)
          .map((e) => ({ offset: e.offset, length: e.length, type: "mentionName", userId: e.userId }));
        tgMention.entities = [];
        closeTgMentionPop(body);
        sent = await invoke("telegram_send_message", { request: { chatId: chatId, text: text, entities: mentionEntities, replyToMessageId: replyToId, threadId: threadId ? Number(threadId) || null : null } });
      }
    } catch (error) {
      if (generation !== telegramRuntime.accountGeneration) return;
      if (input) {
        input.value = text;
        input.setAttribute("aria-label", String(error?.message || error));
      }
      telegramRuntime.drafts.set(chatId, text);
      return;
    } finally {
      if (generation === telegramRuntime.accountGeneration) {
        telegramRuntime.sending = false;
        if (sendBtn) sendBtn.classList.remove("sending");
      }
    }
    if (generation !== telegramRuntime.accountGeneration) return;
    if (!sent) {
      sent = { from: "me", text: text || pendingFile?.split(/[/\\]/).pop() || "", time: "now" };
    }
    if (!sent.time || sent.time === "now") {
      sent.time = String(Math.floor(Date.now() / 1000));
    }
    const cacheKey = telegramMessageCacheKey(chatId, threadId);
    const current = telegramRuntime.messages.get(cacheKey) || [];
    const alreadyCached = sent.id && current.some((m) => m.id === sent.id);
    if (!alreadyCached) cacheTelegramMessages(chatId, [...current, sent], threadId);
    if (config.positions[widgetId]?.activeChatId === chatId && telegramForumThreadId(chatId) === threadId) {
      const scroller = card.querySelector(".tg-messages");
      const exists = sent.id && scroller?.querySelector(`.tg-message[data-message-id="${CSS.escape(String(sent.id))}"]`);
      if (!exists) {
        appendTelegramMessageDom(card, widgetId, chatId, sent);
      }
    }
  };

  body.querySelector(".tg-send")?.addEventListener("click", (e) => {
    e.stopPropagation();
    sendCurrentMessage();
  });
  const composerEl = body.querySelector(".tg-composer");
  if (composerEl && !composerEl._tgDelegated) {
    composerEl._tgDelegated = true;
    composerEl.addEventListener("click", (e) => {
      if (!isCurrent()) return;
      const removeFileBtn = e.target.closest(".tg-pending-file-remove");
      if (removeFileBtn) {
        e.preventDefault();
        e.stopPropagation();
        telegramRuntime.pendingExternalFile = null;
        updateTelegramWidget(card, widgetId);
        return;
      }
      const closeReplyBtn = e.target.closest(".tg-reply-bar-close");
      if (closeReplyBtn) {
        e.preventDefault();
        e.stopPropagation();
        const pos = config.positions[widgetId] || {};
        const curActiveId = pos.activeChatId || active?.id;
        if (curActiveId) telegramRuntime.replyMessage?.delete(curActiveId);
        updateTelegramWidget(card, widgetId);
        return;
      }
      const closeEditBtn = e.target.closest(".tg-edit-bar-close");
      if (closeEditBtn) {
        e.preventDefault();
        e.stopPropagation();
        const pos = config.positions[widgetId] || {};
        const curActiveId = pos.activeChatId || active?.id;
        if (curActiveId) {
          telegramRuntime.editMessage?.delete(curActiveId);
          const input = composerEl.querySelector(".tg-input");
          if (input) input.value = "";
        }
        updateTelegramWidget(card, widgetId);
        return;
      }
    });
    composerEl.addEventListener("pointerdown", (e) => {
      if (e.target.closest(".tg-pending-file-remove, .tg-reply-bar-close, .tg-edit-bar-close, .tg-attach, .tg-send")) {
        e.stopPropagation();
      }
    });
    composerEl.addEventListener("mousedown", (e) => {
      if (e.target.closest(".tg-pending-file-remove, .tg-reply-bar-close, .tg-edit-bar-close, .tg-attach, .tg-send")) {
        e.stopPropagation();
      }
    });
  }
  body.querySelector(".tg-attach")?.addEventListener("click", async (e) => {
    e.stopPropagation();
    if (!isCurrent() || !active?.id) return;
    const chatId = active.id;
    const threadId = boundThreadId;
    const generation = telegramRuntime.accountGeneration;
    
    const path = await invoke("plugin:dialog|open", { options: { multiple: false, title: "Send file" } });
    if (!path || !isCurrent()) return;
    const composer = body.querySelector(".tg-input");
    const draft = (composer?.value || "").trim();
    const caption = draft || "";
    const replying = telegramRuntime.replyMessage?.get(chatId);
    if (replying) telegramRuntime.replyMessage.delete(chatId);
    const replyToId = replying?.id || null;
    try {
      const sent = await invoke("telegram_send_attachment", { request: { chatId, path: path.trim(), caption: caption, replyToMessageId: replyToId, threadId: threadId ? Number(threadId) || null : null } });
      if (generation !== telegramRuntime.accountGeneration) return;
      if (sent) {
        if (!sent.time || sent.time === "now") {
          sent.time = String(Math.floor(Date.now() / 1000));
        }
        const cacheKey = telegramMessageCacheKey(chatId, threadId);
        const current = telegramRuntime.messages.get(cacheKey) || [];
        // Same race as text send: telegram-message-new may already carry this id.
        const alreadyCached = sent.id && current.some((m) => m.id === sent.id);
        if (!alreadyCached) cacheTelegramMessages(chatId, [...current, sent], threadId);
        // Keep the caption draft until TDLib confirms the attachment was sent.
        // If the request fails, the user can correct and retry it.
        if (composer) composer.value = "";
        telegramRuntime.drafts.delete(chatId);
        if (config.positions[widgetId]?.activeChatId === chatId && telegramForumThreadId(chatId) === threadId) {
          updateTelegramWidget(card, widgetId);
        }
      }
    } catch (error) {
      if (generation !== telegramRuntime.accountGeneration) return;
      // Same as text send: attachment failure must not wipe the message list.
      if (composer) composer.setAttribute("aria-label", String(error?.message || error));
    }
  });
  const composerInput = body.querySelector(".tg-input");
  if (composerInput) {
    const sameChatDraft = capturedComposer.chatId && capturedComposer.chatId === (active?.id || null);
    if (sameChatDraft && (capturedComposer.value || capturedComposer.hadFocus)) {
      composerInput.value = capturedComposer.value;
      if (capturedComposer.value) telegramRuntime.drafts.set(active.id, capturedComposer.value);
      else telegramRuntime.drafts.delete(active.id);
      if (capturedComposer.hadFocus) {
        composerInput.focus();
        try {
          composerInput.setSelectionRange(capturedComposer.selectionStart, capturedComposer.selectionEnd);
        } catch (_) { /* ignore selection restore failures */ }
      }
    } else if (active?.id) {
      const draft = telegramRuntime.drafts.get(active.id);
      if (draft) composerInput.value = draft;
    }
    const COMPOSER_MAX_H = 120;
    const autoGrowComposer = () => {
      composerInput.style.height = "auto";
      const nextHeight = Math.max(32, Math.min(composerInput.scrollHeight, COMPOSER_MAX_H));
      composerInput.style.height = `${nextHeight}px`;
      composerInput.style.overflowY = composerInput.scrollHeight > COMPOSER_MAX_H ? "auto" : "hidden";
    };
    autoGrowComposer();
    // Edit-mode drag handling is registered on the canvas. Keep interaction
    // inside the textarea local so a click always focuses the editor.
    // NOTE: Do NOT stopPropagation on pointerdown/mousedown — WebView2 needs
    // the event to bubble for the OS keyboard to attach to the WebView2 window.
    composerInput.addEventListener("pointerdown", (e) => {
      if (document.activeElement !== composerInput) {
        try { composerInput.focus({ preventScroll: true }); } catch { composerInput.focus(); }
      }
    });
    composerInput.addEventListener("mousedown", (e) => {
      if (document.activeElement !== composerInput) {
        try { composerInput.focus({ preventScroll: true }); } catch { composerInput.focus(); }
      }
    });
    composerInput.addEventListener("click", (e) => {
      if (document.activeElement !== composerInput) {
        try { composerInput.focus({ preventScroll: true }); } catch { composerInput.focus(); }
      }
    });
    composerInput.addEventListener("input", () => {
      autoGrowComposer();
      if (!isCurrent() || !active?.id) return;
      const v = composerInput.value;
      if (v) telegramRuntime.drafts.set(active.id, v);
      else telegramRuntime.drafts.delete(active.id);
      scheduleTgMentionFetch(body, active);
    });
    composerInput.addEventListener("keydown", (e) => {
      // Mention popup navigation takes precedence over send/newline.
      if (tgMention.open && tgMention.items.length) {
        if (e.key === "ArrowDown") {
          e.preventDefault();
          tgMention.index = (tgMention.index + 1) % tgMention.items.length;
          renderTgMentionPop(body);
          return;
        }
        if (e.key === "ArrowUp") {
          e.preventDefault();
          tgMention.index = (tgMention.index - 1 + tgMention.items.length) % tgMention.items.length;
          renderTgMentionPop(body);
          return;
        }
        if (e.key === "Enter" || e.key === "Tab") {
          e.preventDefault();
          pickTgMention(body);
          return;
        }
        if (e.key === "Escape") {
          e.preventDefault();
          e.stopPropagation();
          closeTgMentionPop(body);
          return;
        }
      }
      if (e.key === "Enter" && !e.shiftKey) {
        e.preventDefault();
        sendCurrentMessage();
      }
      // Shift+Enter: default textarea behavior inserts a newline.
    });
    const mentionPop = body.querySelector(".tg-mention-pop");
    if (mentionPop) {
      mentionPop.addEventListener("pointerdown", (e) => e.stopPropagation());
      mentionPop.addEventListener("mousedown", (e) => e.stopPropagation());
      mentionPop.addEventListener("click", (e) => {
        e.stopPropagation();
        const item = e.target.closest(".tg-mention-item");
        if (!item) return;
        tgMention.index = Number(item.dataset.mentionIndex) || 0;
        pickTgMention(body);
      });
      // Popup state survives lightweight updates but not full rebuilds.
      if (tgMention.open && tgMention.chatId === (active?.id || "")) renderTgMentionPop(body);
    }
  }

}

function renderTelegramWidget(card, widgetId, { preparing = false } = {}) {
  cancelActiveTelegramMorph(card);
  if (card?._tgSurfaceTransitioning) {
    card._tgPendingUpdate = true;
    return;
  }
  card.classList.add("telegram-widget", "no-header");
  const previousMessages = card.querySelector(".tg-messages");
  const dockedScroll = preparing && card._tgDockedScroll?.generation === telegramRuntime.accountGeneration
    ? card._tgDockedScroll : null;
  const prevChatId = previousMessages?.dataset.chatId || dockedScroll?.chatId || null;
  const prevThreadId = previousMessages?.dataset.threadId || dockedScroll?.threadId || null;
  const previousScroll = previousMessages ? captureTelegramScroll(previousMessages) : dockedScroll?.state || null;
  const previousList = card.querySelector(".tg-chat-list");
  const previousListScroll = previousList ? captureTelegramListScroll(previousList) : null;
  const pos = config.positions[widgetId] || (config.positions[widgetId] = {});
  const canvasW = widgetsCanvas.clientWidth || window.innerWidth || 1920;
  if (pos.x == null || pos.x > canvasW - 24) {
    pos.x = Math.max(0, canvasW - (pos.w || 56) - 12);
    config.positions[widgetId] = pos;
  }
  if (!card.classList.contains("expanded")) {
    card.style.left = pos.x + "px";
    card.style.top = (pos.y ?? 118) + "px";
    card.style.width = (pos.w || 56) + "px";
    card.style.height = (pos.h || 520) + "px";
  }

  const body = card.querySelector(".widget-body");
  const chats = telegramVisibleChats();
  const activeId = pos.activeChatId || chats.find((chat) => chat.unread)?.id || chats[0]?.id;
  const active = chats.find((chat) => chat.id === activeId) || chats[0];
  pos.activeChatId = active?.id;
  const status = telegramRuntime.status;
  const showAuth = (pos.showSettings === true) || shouldShowTelegramAuth(status);
  const expanded = card.classList.contains("expanded") || pos.expanded === true;
  const activeThreadId = telegramForumThreadId(active?.id);
  const activeMessageKey = telegramMessageCacheKey(active?.id, activeThreadId);
  if (expanded && active && !telegramRuntime.messages.has(activeMessageKey)) {
    cacheTelegramMessages(active.id, []);
  }

  const msgKey = telegramMessageCacheKey(active?.id, activeThreadId);
  const activeMessages = telegramRuntime.messages.get(msgKey) || [];
  const railHtml = buildRailHtml(chats, active?.id);
  // Forum chats / communities keep the collapsed chat-list + topics column layout.
  const forumChatId = telegramRuntime.forumOpenChatId;
  const forumChat = forumChatId ? chats.find((c) => c.id === forumChatId) : null;
  const showForumTopics = expanded && !!forumChat;
  let listHtml = expanded ? buildChatListHtml(chats, active?.id, telegramChatListState(chats)) : "";
  let topicsHtml = "";
  if (showForumTopics) {
    topicsHtml = buildForumTopicsListHtml(forumChat);
    if (!telegramRuntime.forumTopics.has(forumChat.id)) {
      loadForumTopics(forumChat.id);
    }
  }
  const showSettings = pos.showSettings === true;
  const threadInnerHtml = expanded ? buildThreadHtml(active, activeMessages, showAuth, status, showSettings) : "";
  // Forum chat with no topic selected: thread shows a hint (topics in column).
  const showTopicHint = expanded && !showAuth && showForumTopics && active?.id === forumChat?.id && !telegramForumThreadId(forumChat.id);
  const effectiveThreadInnerHtml = showTopicHint
    ? `<div class="tg-thread-head">
        <span class="tg-thread-avatar" data-initials="${escapeHtml(active.initials)}">${telegramAvatarInner(active)}</span>
        <div class="tg-thread-head-info"><strong>${escapeHtml(active.title)}</strong></div>
      </div>
      <div class="tg-messages"><div class="tg-state">Select a topic</div></div>`
    : threadInnerHtml;
  const messagesInnerHtml = expanded && !showAuth
    ? (showTopicHint ? '<div class="tg-state">Select a topic</div>' : buildMessagesInnerHtml(active, activeMessages)) : "";
  card._tgSectionCache = { rail: railHtml, list: listHtml, thread: messagesInnerHtml, topics: topicsHtml, topicsChatId: forumChat?.id || null };

  // Preserve composer draft/focus across full-widget rebuilds (polls, message sync).
  const prevInput = body.querySelector(".tg-input");
  const capturedComposer = {
    chatId: prevChatId,
    value: prevInput ? prevInput.value : "",
    hadFocus: !!(prevInput && document.activeElement === prevInput),
    selectionStart: prevInput?.selectionStart ?? 0,
    selectionEnd: prevInput?.selectionEnd ?? 0,
  };
  // Same pattern for auth/settings inputs (phone, code, 2FA, API form, etc.).
  const capturedAuthFields = captureTelegramAuthFields(body);

  const accounts = status?.accounts || [];
  const activeAccountId = status?.activeAccountId || "default";
  const switcherHtml = buildAccountSwitcherHtml(accounts, activeAccountId);

  const collapsedMarkup = `
    <button type="button" class="tg-rail-toggle" aria-label="Telegram widget">
      <span></span>
    </button>
    <div class="tg-rail">${railHtml}</div>
  `;
  const expandedMarkup = `
    <section class="tg-panel">
        <div class="tg-panel-header">
          <div class="tg-panel-header-left">
            <strong>Telegram</strong>
            ${switcherHtml}
          </div>
          <div class="tg-panel-header-actions">
            <button type="button" class="tg-settings${pos.showSettings ? " active" : ""}" aria-label="Settings">
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
                <circle cx="12" cy="12" r="3"></circle>
                <path d="M19.4 15a1.65 1.65 0 0 0 .33 1.82l.06.06a2 2 0 0 1 0 2.83 2 2 0 0 1-2.83 0l-.06-.06a1.65 1.65 0 0 0-1.82-.33 1.65 1.65 0 0 0-1 1.51V21a2 2 0 0 1-2 2 2 2 0 0 1-2-2v-.09A1.65 1.65 0 0 0 9 19.4a1.65 1.65 0 0 0-1.82.33l-.06.06a2 2 0 0 1-2.83 0 2 2 0 0 1 0-2.83l.06-.06a1.65 1.65 0 0 0 .33-1.82 1.65 1.65 0 0 0-1.51-1H3a2 2 0 0 1-2-2 2 2 0 0 1 2-2h.09A1.65 1.65 0 0 0 4.6 9a1.65 1.65 0 0 0-.33-1.82l-.06-.06a2 2 0 0 1 0-2.83 2 2 0 0 1 2.83 0l.06.06a1.65 1.65 0 0 0 1.82.33H9a1.65 1.65 0 0 0 1-1.51V3a2 2 0 0 1 2-2 2 2 0 0 1 2 2v.09a1.65 1.65 0 0 0 1 1.51 1.65 1.65 0 0 0 1.82-.33l.06-.06a2 2 0 0 1 2.83 0 2 2 0 0 1 0 2.83l-.06.06a1.65 1.65 0 0 0-.33 1.82V9a1.65 1.65 0 0 0 1.51 1H21a2 2 0 0 1 2 2 2 2 0 0 1-2 2h-.09a1.65 1.65 0 0 0-1.51 1z"></path>
              </svg>
            </button>
            <button type="button" class="tg-collapse" aria-label="Collapse">
              <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
                <path d="M15 18l-6-6 6-6"/>
              </svg>
            </button>
          </div>
        </div>
        ${telegramConnectionStripHtml()}
        <div class="tg-layout ${showForumTopics ? "forum-topics" : ""}">
          ${buildChatListToolsHtml()}
          <div class="tg-chat-list">
            ${listHtml}
          </div>
          <div class="tg-topics-list">
            ${topicsHtml}
          </div>
          <div class="tg-thread ${showAuth ? "auth" : ""}">
            ${effectiveThreadInnerHtml}
          </div>
        </div>
    </section>
  `;
  body.innerHTML = `
    <div class="tg-shell">
      ${expanded ? expandedMarkup : collapsedMarkup}
    </div>
  `;
  syncTelegramTopicsVisibility(body, showForumTopics);
  // Preparation owns the exact final DOM. Activation below only installs
  // handlers/effects after the surface is visible, without replacing its nodes.
  const activate = () => {
  wireTelegramRailSection(card, widgetId, body);
  wireTelegramListSection(card, widgetId, body);
  wireTelegramTopicsSection(card, widgetId, body, active);
  wireTelegramContextMenus(card, widgetId, body);
  wireTelegramMessagesSection(card, widgetId, body, active, {
    previousScroll,
    prevChatId,
    prevThreadId,
    showAuth,
    restoreScroll: true,
  });
  restoreTelegramListScroll(body.querySelector(".tg-chat-list"), previousListScroll);
  restoreTelegramAuthFields(body, capturedAuthFields);
  wireTelegramAccountSwitcher(card, widgetId, body, activeAccountId);

  // Thread-head avatar lives outside the messages section.
  wireTelegramAvatarErrors(body.querySelector(".tg-thread-head"));
  kickTelegramAvatarDownloads(card, widgetId, chats);

  body.querySelector(".tg-rail-toggle")?.addEventListener("click", (e) => {
    e.stopPropagation();
    if (editMode) return;
    toggleTelegramWidget(card, widgetId);
  });
  card.querySelector(".tg-settings")?.addEventListener("click", async (e) => {
    e.stopPropagation();
    if (editMode) return;
    const originBtn = e.currentTarget;
    const willShowSettings = !config.positions[widgetId].showSettings;
    config.positions[widgetId].showSettings = willShowSettings;
    await saveConfig();

    const panelEl = card.querySelector(".tg-panel");
    const oldThread = card.querySelector(".tg-thread");
    const isExpanded = card.classList.contains("expanded");

    if (isExpanded && panelEl && oldThread && (originBtn instanceof HTMLElement) && oldThread.offsetWidth > 0) {
      const chats = telegramVisibleChats();
      const activeId = config.positions[widgetId]?.activeChatId;
      const active = chats.find((c) => c.id === activeId) || chats[0];
      const msgKey = telegramMessageCacheKey(active?.id);
      const activeMessages = telegramRuntime.messages.get(msgKey) || [];
      const showAuth = willShowSettings || shouldShowTelegramAuth(telegramRuntime.status);
      const targetThreadHtml = buildThreadHtml(active, activeMessages, showAuth, telegramRuntime.status, willShowSettings);

      originBtn.classList.toggle("active", willShowSettings);

      telegramMorphToThread(card, widgetId, originBtn, targetThreadHtml, () => {
        renderTelegramWidget(card, widgetId);
      });
    } else {
      cancelActiveTelegramMorph(card);
      renderTelegramWidget(card, widgetId);
    }
  });
  card.querySelector(".tg-collapse")?.addEventListener("click", (e) => {
    e.stopPropagation();
    if (editMode) return;
    collapseTelegramWidget(card, widgetId);
  });
  const chatSearch = body.querySelector(".tg-chat-search");
  body.querySelector("[data-tg-action='toggle-archive']")?.addEventListener("click", async (event) => {
    event.stopPropagation();
    telegramRuntime.chatListView = telegramRuntime.chatListView === "archive" ? "main" : "archive";
    telegramRuntime.chatVisibleLimit = TELEGRAM_CHAT_PAGE_SIZE;
    telegramRuntime.chatFilter = "";
    if (telegramRuntime.chatListView === "archive") {
      await refreshTelegramArchiveChats(card, widgetId);
    }
    renderTelegramWidget(card, widgetId);
  });
  if (chatSearch) {
    // Do NOT stopPropagation on pointerdown/mousedown — WebView2 needs the
    // event to bubble for the OS keyboard to attach to the WebView2 window.
    // Manually focus as a safety net only.
    const tgFocusSearch = () => {
      if (document.activeElement !== chatSearch) {
        try { chatSearch.focus({ preventScroll: true }); } catch { chatSearch.focus(); }
      }
    };
    chatSearch.addEventListener("pointerdown", tgFocusSearch);
    chatSearch.addEventListener("mousedown", tgFocusSearch);
  }
  chatSearch?.addEventListener("input", (e) => {
    telegramRuntime.chatFilter = e.target.value;
    telegramRuntime.chatVisibleLimit = TELEGRAM_CHAT_PAGE_SIZE;
    const listEl = body.querySelector(".tg-chat-list");
    if (!listEl) return;
    // Read live chats — do not close over the render-time snapshot (empty if
    // the panel opened before the first chats load finished).
    const liveChats = telegramVisibleChats();
    const liveActiveId = config.positions[widgetId]?.activeChatId || active?.id;
    const nextList = buildChatListHtml(liveChats, liveActiveId, telegramChatListState(liveChats));
    const listScroll = captureTelegramListScroll(listEl);
    listEl.innerHTML = nextList;
    if (card._tgSectionCache) card._tgSectionCache.list = nextList;
    wireTelegramListSection(card, widgetId, body);
    restoreTelegramListScroll(listEl, listScroll);
  });
  wireTelegramComposer(card, widgetId, body.querySelector(".tg-thread"), active, capturedComposer);
  body.querySelector(".tg-auth-phone")?.addEventListener("click", async (e) => {
    e.stopPropagation();
    const phoneNumber = body.querySelector('.tg-auth-field[data-tg-auth="phone"]')?.value?.trim();
    if (!phoneNumber) return;
    await runTelegramAuthStep(() => invoke("telegram_set_phone", { phone: { phoneNumber: phoneNumber } }), card, widgetId);
  });
  body.querySelector(".tg-auth-code")?.addEventListener("click", async (e) => {
    e.stopPropagation();
    const code = body.querySelector('.tg-auth-field[data-tg-auth="code"]')?.value?.trim();
    if (!code) return;
    await runTelegramAuthStep(() => invoke("telegram_check_code", { code: { code: code } }), card, widgetId);
  });
  body.querySelector(".tg-auth-password")?.addEventListener("click", async (e) => {
    e.stopPropagation();
    const password = body.querySelector('.tg-auth-field[data-tg-auth="password"]')?.value || "";
    if (!password) return;
    await runTelegramAuthStep(() => invoke("telegram_check_password", { password: { password: password } }), card, widgetId);
  });
  body.querySelector(".tg-auth-email")?.addEventListener("click", async (e) => {
    e.stopPropagation();
    const emailAddress = body.querySelector('.tg-auth-field[data-tg-auth="email"]')?.value?.trim();
    if (!emailAddress) return;
    await runTelegramAuthStep(() => invoke("telegram_set_email", { email: { emailAddress: emailAddress } }), card, widgetId);
  });
  body.querySelector(".tg-auth-email-code")?.addEventListener("click", async (e) => {
    e.stopPropagation();
    const code = body.querySelector('.tg-auth-field[data-tg-auth="emailCode"]')?.value?.trim();
    if (!code) return;
    await runTelegramAuthStep(() => invoke("telegram_check_email_code", { code: { code: code } }), card, widgetId);
  });
  body.querySelector(".tg-auth-register")?.addEventListener("click", async (e) => {
    e.stopPropagation();
    const firstName = body.querySelector('.tg-auth-field[data-tg-auth="firstName"]')?.value?.trim();
    const lastName = body.querySelector('.tg-auth-field[data-tg-auth="lastName"]')?.value?.trim() || "";
    if (!firstName) return;
    await runTelegramAuthStep(
      () => invoke("telegram_register_user", { registration: { firstName: firstName, lastName: lastName } }),
      card,
      widgetId,
    );
  });
  body.querySelectorAll(".tg-auth-field").forEach((field) => {
    // NOTE: Do NOT stopPropagation on pointerdown/mousedown — WebView2 needs
    // the event to bubble for the OS keyboard to attach to the WebView2 window.
    // The drag handlers on the canvas should check if the target is an input.
    field.addEventListener("pointerdown", (e) => {
      if (document.activeElement !== field) {
        try { field.focus({ preventScroll: true }); } catch { field.focus(); }
      }
    });
    field.addEventListener("mousedown", (e) => {
      if (document.activeElement !== field) {
        try { field.focus({ preventScroll: true }); } catch { field.focus(); }
      }
    });
  });
  body.querySelectorAll(".tg-auth-field[data-tg-auth]").forEach((field) => {
    field.addEventListener("keydown", (e) => {
      if (e.key !== "Enter") return;
      e.preventDefault();
      body.querySelector(
        ".tg-auth-phone, .tg-auth-code, .tg-auth-password, .tg-auth-email, .tg-auth-email-code, .tg-auth-register",
      )?.click();
    });
  });
  body.querySelector(".tg-auth-copy-link")?.addEventListener("click", async (e) => {
    e.stopPropagation();
    const link = e.currentTarget.dataset.link || "";
    if (!link) return;
    try {
      await navigator.clipboard?.writeText?.(link);
      e.currentTarget.textContent = "Copied";
      setTimeout(() => {
        if (e.currentTarget.isConnected) e.currentTarget.textContent = "Copy link";
      }, 1500);
    } catch (_) { /* clipboard may be denied */ }
  });
  body.querySelector(".tg-auth-logout")?.addEventListener("click", async (e) => {
    e.stopPropagation();
    const btn = e.currentTarget;
    if (!btn.classList.contains("confirming")) {
      btn.classList.add("confirming");
      btn.textContent = "Log out?";
      const token = Symbol("tg-logout-confirm");
      btn._tgLogoutConfirmToken = token;
      setTimeout(() => {
        if (btn._tgLogoutConfirmToken !== token) return;
        btn.classList.remove("confirming");
        btn.textContent = "Log Out";
        delete btn._tgLogoutConfirmToken;
      }, 4000);
      return;
    }
    try {
      await invoke("telegram_logout");
      if (config.positions[widgetId]) {
        config.positions[widgetId].showSettings = false;
        config.positions[widgetId].activeChatId = null;
      }
      clearTelegramAccountState();
      await saveConfig();
      telegramRuntime.statusLoaded = false;
      refreshTelegramStatus(card, widgetId);
    } catch (err) {
      console.error("Logout failed:", err);
    }
  });
  body.querySelector(".tg-auth-save-config")?.addEventListener("click", async (e) => {
    e.stopPropagation();
    await saveTelegramConfigFromAuthForm(body, card, widgetId);
  });
  // The managed ByeDPI desync needs no host/port/credentials — those fields
  // only apply to manual socks5/http proxies.
  body.querySelector('[data-tg-config="proxyType"]')?.addEventListener("change", (e) => {
    const managed = e.target.value === "byedpi";
    for (const key of ["proxyHost", "proxyPort", "proxyUsername", "proxyPassword"]) {
      const field = body.querySelector(`[data-tg-config="${key}"]`);
      if (field) field.disabled = managed;
    }
  });
  body.querySelector(".tg-auth-restart")?.addEventListener("click", async (e) => {
    e.stopPropagation();
    await runTelegramAuthStep(() => invoke("telegram_restart"), card, widgetId);
    telegramRuntime.statusLoaded = false;
    telegramRuntime.debugLoaded = false;
    renderTelegramWidget(card, widgetId);
  });
  body.querySelectorAll(".tg-settings-account-switch").forEach((btn) => {
    btn.addEventListener("click", async (e) => {
      e.stopPropagation();
      const accId = btn.dataset.accId;
      if (accId) {
        const bodyEl = card.querySelector(".widget-body");
        if (bodyEl) bodyEl.style.opacity = "0.4";
        clearTelegramAccountState();
        if (config.positions[widgetId]) {
          config.positions[widgetId].activeChatId = null;
        }
        await saveConfig();
        try {
          await invoke("telegram_switch_account", { accountId: accId });
        } catch (err) {
          console.error("switch account error:", err);
        }
        telegramRuntime.statusLoaded = false;
        telegramRuntime.chatsLoaded = false;
        await refreshTelegramStatus(card, widgetId);
        await refreshTelegramChats(card, widgetId, true);
        if (bodyEl) bodyEl.style.opacity = "";
      }
    });
  });
  body.querySelectorAll(".tg-settings-account-remove").forEach((btn) => {
    btn.addEventListener("click", async (e) => {
      e.stopPropagation();
      const accId = btn.dataset.accId;
      if (accId) {
        clearTelegramAccountState();
        if (config.positions[widgetId]) {
          config.positions[widgetId].activeChatId = null;
        }
        await saveConfig();
        try {
          await invoke("telegram_remove_account", { accountId: accId });
        } catch (err) {
          console.error("remove account error:", err);
        }
        telegramRuntime.statusLoaded = false;
        refreshTelegramStatus(card, widgetId);
      }
    });
  });
  body.querySelector(".tg-settings-account-add-btn")?.addEventListener("click", async (e) => {
    e.stopPropagation();
    clearTelegramAccountState();
    if (config.positions[widgetId]) {
      config.positions[widgetId].showSettings = false;
      config.positions[widgetId].activeChatId = null;
    }
    await saveConfig();
    try {
      await invoke("telegram_add_account");
    } catch (err) {
      console.error("add account error:", err);
    }
    telegramRuntime.statusLoaded = false;
    refreshTelegramStatus(card, widgetId);
  });

  refreshTelegramStatus(card, widgetId);
  refreshTelegramConfig(card, widgetId);
  refreshTelegramDebug(card, widgetId);
  refreshTelegramChats(card, widgetId);
  if (expanded && active?.id) refreshTelegramMessages(card, widgetId, active.id, true);
  setupTelegramListeners(card, widgetId);

  // Restore expanded geometry from saved pos (startup / remount). Class check
  // avoids re-expanding on every poll/re-render once the card is already open.
  // Never open during edit/move mode — layout only. The card is a surfaceManager
  // surface now, so an in-flight open/close must also block a duplicate toggle.
  if (pos.expanded && !card.classList.contains("expanded") && !surfaceManager.isOpen(card) && !editMode) {
    toggleTelegramWidget(card, widgetId);
  }
  };
  if (preparing) {
    const scroller = body.querySelector('.tg-messages');
    if (scroller) {
      if (previousScroll && prevChatId === active?.id && String(prevThreadId || '') === String(activeThreadId || '')) {
        restoreTelegramScroll(scroller, previousScroll);
      } else scroller.scrollTop = scroller.scrollHeight;
    }
    const generation = telegramRuntime.accountGeneration;
    card._tgActivatePrepared = () => {
      if (generation !== telegramRuntime.accountGeneration) {
        renderTelegramWidget(card, widgetId);
        return;
      }
      activate();
    };
  } else {
    activate();
  }
}

function syncTelegramTopicsVisibility(body, visible) {
  const topics = body.querySelector('.tg-topics-list');
  if (!topics) return;
  if (!visible && topics.contains(document.activeElement)) {
    body.querySelector('.tg-chat-search')?.focus({ preventScroll: true });
  }
  topics.classList.toggle('hidden', !visible);
  topics.inert = !visible;
  topics.setAttribute('aria-hidden', String(!visible));
}

// .tg-messages is not positioned, so message offsetTop is relative to .tg-panel
// (header + thread head inflate it). Measure in the scroller's content frame.
function tgMessageOffsetInScroller(item, scroller) {
  return item.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop;
}

function captureTelegramListScroll(list) {
  if (!list) return null;
  const top = list.getBoundingClientRect().top;
  const row = [...list.querySelectorAll(".tg-chat-row")]
    .find((item) => item.getBoundingClientRect().bottom > top + 1);
  return {
    scrollTop: list.scrollTop,
    chatId: row?.dataset.chatId || null,
    offset: row ? row.getBoundingClientRect().top - top : 0,
  };
}

function restoreTelegramListScroll(list, state) {
  if (!list || !state) return;
  list._tgRestoringScroll = true;
  if (state.chatId) {
    const row = [...list.querySelectorAll(".tg-chat-row")]
      .find((item) => item.dataset.chatId === state.chatId);
    if (row) {
      const listTop = list.getBoundingClientRect().top;
      list.scrollTop = Math.max(0, row.getBoundingClientRect().top - listTop + list.scrollTop - state.offset);
    } else {
      list.scrollTop = Math.min(state.scrollTop, Math.max(0, list.scrollHeight - list.clientHeight));
    }
  } else {
    list.scrollTop = Math.min(state.scrollTop, Math.max(0, list.scrollHeight - list.clientHeight));
  }
  requestAnimationFrame(() => {
    if (list.isConnected) list._tgRestoringScroll = false;
  });
}

function captureTelegramScroll(scroller) {
  const scrollTop = scroller.scrollTop;
  const atLatest = scroller.scrollHeight - scrollTop - scroller.clientHeight <= 24;
  const messages = [...scroller.querySelectorAll(".tg-message[data-message-id]")];
  const viewportTop = scrollTop + 1;
  const anchor = messages.find((item) => {
    const top = tgMessageOffsetInScroller(item, scroller);
    return top + item.offsetHeight > viewportTop;
  }) || messages[0];
  return {
    scrollTop,
    atLatest,
    messageId: anchor?.dataset.messageId || "",
    offset: anchor ? tgMessageOffsetInScroller(anchor, scroller) - scrollTop : 0,
  };
}

function restoreTelegramScroll(scroller, state) {
  if (state.atLatest) {
    scroller.scrollTop = Math.max(0, scroller.scrollHeight - scroller.clientHeight);
    return;
  }
  if (state.messageId) {
    const anchor = scroller.querySelector(`[data-message-id="${CSS.escape(state.messageId)}"]`);
    if (anchor) {
      scroller.scrollTop = Math.max(0, tgMessageOffsetInScroller(anchor, scroller) - state.offset);
      return;
    }
  }
  const maxScroll = Math.max(0, scroller.scrollHeight - scroller.clientHeight);
  scroller.scrollTop = Math.min(Math.max(0, state.scrollTop), maxScroll);
}

// One immediate restore plus one guarded geometry settle. A stale frame can
// never release another update's suppression or undo a wheel/key/touch gesture.
function settleTelegramScroll(scroller) {
  const revision = (scroller._tgScrollRevision || 0) + 1;
  scroller._tgScrollRevision = revision;
  scroller._tgSuppressAutoLoad = true;
  const state = captureTelegramScroll(scroller);
  scroller._tgReadingPosition = state;
  const height = scroller.scrollHeight;
  const top = scroller.scrollTop;
  const key = `${scroller.dataset.chatId || ''}/${scroller.dataset.threadId || ''}`;
  if (!scroller._tgScrollGuardWired) {
    scroller._tgScrollGuardWired = true;
    const cancel = () => {
      scroller._tgScrollRevision = (scroller._tgScrollRevision || 0) + 1;
      scroller._tgSuppressAutoLoad = false;
    };
    scroller.addEventListener('wheel', cancel, { passive: true });
    scroller.addEventListener('touchstart', cancel, { passive: true });
    scroller.addEventListener('pointerdown', cancel, { passive: true });
    scroller.addEventListener('keydown', cancel);
  }
  requestAnimationFrame(() => {
    if (!scroller.isConnected || scroller._tgScrollRevision !== revision ||
        `${scroller.dataset.chatId || ''}/${scroller.dataset.threadId || ''}` !== key) return;
    if (Math.abs(scroller.scrollTop - top) < 1 && scroller.scrollHeight !== height) {
      restoreTelegramScroll(scroller, state);
    }
    scroller._tgReadingPosition = captureTelegramScroll(scroller);
    scroller._tgSuppressAutoLoad = false;
  });
}

async function handleTelegramAttachmentDownload(card, widgetId, chatId, messageId, fileId, openAfter) {
  if (!chatId || !fileId) return;
  const downloads = (telegramRuntime.mediaDownloads ||= new Set());
  const key = String(fileId);
  if (downloads.has(key)) return;
  downloads.add(key);
  try {
    // Backend is non-blocking: non-empty path = already on disk; empty = async
    // download started and path will land via updateFile + telegram-attachment-ready.
    const path = await invoke("telegram_download_file", { fileId: Number(fileId) });
    telegramRuntime.mediaFailed?.delete(key);
    if (path) {
      applyTelegramAttachmentReady({ chatId, messageId, path });
      if (openAfter) {
        const opened = await (async () => { try { await invoke("open_explorer", { path: path }); } catch { await openPath(path); } return true; })();
        if (opened === false) {
          telegramRuntime.messagesError.set(telegramMessageCacheKey(chatId), "Could not open file (blocked by app permissions)");
          updateTelegramWidget(card, widgetId);
        }
      }
    }
    // Empty path: leave the pending placeholder; telegram-attachment-ready
    // will patch the row once TDLib reports the local file.
  } catch (error) {
    // Remember the failure so the auto-download pass doesn't retry in a loop;
    // the message gets a manual "Retry" button instead.
    (telegramRuntime.mediaFailed ||= new Set()).add(key);
    updateTelegramWidget(card, widgetId);
  } finally {
    downloads.delete(key);
  }
}

/** Apply a completed attachment path to the in-memory message and, if that
 *  chat's thread is open, swap the placeholder for the real media DOM. */
function applyTelegramAttachmentReady(payload) {
  const chatId = payload?.chatId;
  const messageId = payload?.messageId;
  const path = payload?.path;
  if (!chatId || !messageId || !path) return;

  const cacheKey = telegramCachedMessageKey(chatId, messageId);
  const list = telegramRuntime.messages.get(cacheKey) || [];
  const index = list.findIndex((message) => String(message.id) === String(messageId));
  if (index === -1) return;

  const updated = { ...list[index], attachmentLocalPath: path };
  list[index] = updated;
  const threadId = cacheKey.startsWith(`${chatId}::`) ? cacheKey.slice(String(chatId).length + 2) : null;
  cacheTelegramMessages(chatId, [...list], threadId);
  telegramRuntime.messages.set(cacheKey, [...list]);
  telegramRuntime.mediaFailed?.delete(String(updated.attachmentFileId || ""));

  const card = getTelegramCard();
  if (!card) return;
  const widgetId = card.dataset.id;
  const activeId = config.positions?.[widgetId]?.activeChatId;
  if (activeId !== chatId || telegramMessageCacheKey(chatId) !== cacheKey) return;

  const row = card.querySelector(`.tg-message[data-message-id="${CSS.escape(String(messageId))}"]`);
  if (!row) {
    // Thread open but row missing (e.g. filtered) — fall back to scoped re-render.
    queueTelegramRender(card, widgetId);
    return;
  }

  // Reuse the same markup path used when the file is already downloaded.
  const html = telegramAttachmentHtml(updated, chatId);
  if (!html) return;
  const existing = row.querySelector(".tg-media, .tg-media-pending, .tg-attachment, .tg-attachment-load, .tg-attachment-download, .tg-attachment-open");
  const temp = document.createElement("div");
  temp.innerHTML = html;
  const node = temp.firstElementChild;
  if (!node) return;
  const scroller = row.closest('.tg-messages');
  const readingPosition = scroller ? captureTelegramScroll(scroller) : null;
  if (existing) existing.replaceWith(node);
  else {
    const small = row.querySelector(":scope > small");
    if (small) row.insertBefore(node, small);
    else row.appendChild(node);
  }
  if (readingPosition) {
    restoreTelegramScroll(scroller, readingPosition);
    settleTelegramScroll(scroller);
  }
  wireTelegramAnimationControls(row);
  if (node.classList.contains("tg-attachment-open")) {
    node.addEventListener("click", async (e) => {
      e.stopPropagation();
      const opened = await (async () => { try { await invoke("open_explorer", { path: node.dataset.path }); } catch { await openPath(node.dataset.path); } return true; })();
      if (opened === false) {
        telegramRuntime.messagesError.set(telegramMessageCacheKey(chatId), "Could not open file (blocked by app permissions)");
        updateTelegramWidget(card, widgetId);
      }
    });
  }
  if (scroller) syncTelegramVisibleMedia(card, widgetId, scroller);
}

// Keep decoded image/video surfaces close to the viewport. Large decoded media
// primarily lives in the WebView GPU process, so autoplaying every GIF in a long
// thread can consume hundreds of MB even when most messages are off-screen.
function syncTelegramVisibleMedia(card, widgetId, scroller) {
  if (!scroller?.isConnected || !card?.classList.contains("expanded")) return;
  const viewport = scroller.getBoundingClientRect();
  const margin = Math.max(160, viewport.height);
  const isNear = (el) => {
    const rect = el.getBoundingClientRect();
    return rect.bottom >= viewport.top - margin && rect.top <= viewport.bottom + margin;
  };

  // Autoplay only muted GIF/animation loops — skip user-controlled video players.
  scroller.querySelectorAll("video.tg-animation[muted]").forEach((video) => {
    const animationKey = video.closest(".tg-animation-wrap")?.dataset.animationKey || "";
    const userPaused = video._tgUserPaused || telegramRuntime.pausedAnimations.has(animationKey);
    if (userPaused) {
      if (!video.paused) video.pause();
    } else if (isNear(video)) {
      if (video.paused) video.play().catch(() => {});
    } else if (!video.paused) {
      video.pause();
    }
  });
  scroller.querySelectorAll(".tg-media-pending").forEach((el) => {
    if (!isNear(el)) return;
    handleTelegramAttachmentDownload(
      card,
      widgetId,
      el.dataset.chatId,
      el.dataset.messageId,
      el.dataset.fileId,
      false,
    );
  });
}

async function loadTelegramOlderMessages(card, widgetId, chatId, requestedThreadId = telegramForumThreadId(chatId)) {
  const threadId = requestedThreadId || null;
  const cacheKey = telegramMessageCacheKey(chatId, threadId);
  if (telegramRuntime.loadingEarlier.has(cacheKey)) return;
  if (telegramRuntime.historyExhausted.has(cacheKey)) return;
  if (Date.now() < (telegramRuntime.earlierCooldownUntil.get(cacheKey) || 0)) return;
  const current = telegramRuntime.messages.get(cacheKey) || [];
  // The durable TDLib database remains complete; the renderer deliberately
  // keeps only a bounded window so long chats cannot grow WebView without end.
  if (current.length >= TELEGRAM_MAX_CACHED_MESSAGES) return;
  const oldest = current.find((message) => message.id);
  if (!oldest?.id) return;
  const generation = telegramRuntime.accountGeneration;
  telegramRuntime.loadingEarlier.add(cacheKey);
  try {
    const messages = await invoke("telegram_older_messages", {
      chatId,
      beforeMessageId: oldest.id,
      threadId: threadId ? Number(threadId) || null : null,
    });
    if (generation !== telegramRuntime.accountGeneration) return;
    if (!Array.isArray(messages) || !messages.length) {
      telegramRuntime.historyExhausted.add(cacheKey);
      telegramRuntime.earlierCooldownUntil.set(cacheKey, Date.now() + 60000);
      return;
    }
    const known = new Set(current.map((message) => message.id));
    const older = messages.filter((message) => !known.has(message.id));
    if (!older.length) {
      telegramRuntime.historyExhausted.add(cacheKey);
      telegramRuntime.earlierCooldownUntil.set(cacheKey, Date.now() + 60000);
      return;
    }
    telegramRuntime.earlierCooldownUntil.delete(cacheKey);
    const isStillVisible = config.positions[widgetId]?.activeChatId === chatId
      && telegramForumThreadId(chatId) === threadId;
    if (isStillVisible) {
      telegramRuntime.pendingScrollAnchor = { chatId, threadId, messageId: oldest.id };
    }
    const total = [...older, ...current];
    cacheTelegramMessages(chatId, total, threadId);
    if (isStillVisible) updateTelegramWidget(card, widgetId);
    if (total.length < 25 && !telegramRuntime.historyExhausted.has(cacheKey) && isStillVisible) {
      loadTelegramOlderMessages(card, widgetId, chatId, threadId);
    }
  } catch {
    // Older history is best-effort; leave the current thread intact on failure.
  } finally {
    if (generation === telegramRuntime.accountGeneration) telegramRuntime.loadingEarlier.delete(cacheKey);
  }
}

async function runTelegramAuthStep(action, card, widgetId) {
  if (telegramRuntime.authSubmitting) return;
  telegramRuntime.authSubmitting = true;
  try {
    if (false) {
      throw new Error("Tauri backend is not available in live preview. Run the Tauri app for real Telegram login.");
    }
    const status = await action();
    if (status) {
      telegramRuntime.status = status;
      telegramRuntime.statusLoaded = true;
    }
  } catch (error) {
    telegramRuntime.status = {
      mode: "tdlib",
      authorized: false,
      authState: "error",
      authHint: String(error?.message || error),
    };
  } finally {
    telegramRuntime.authSubmitting = false;
  }
  renderTelegramWidget(card, widgetId);
}

async function saveTelegramConfigFromAuthForm(body, card, widgetId) {
  const config = {
    apiId: body.querySelector('[data-tg-config="apiId"]')?.value?.trim() || "",
    apiHash: body.querySelector('[data-tg-config="apiHash"]')?.value?.trim() || "",
    tdjsonPath: body.querySelector('[data-tg-config="tdjsonPath"]')?.value?.trim() || "",
    proxyType: body.querySelector('[data-tg-config="proxyType"]')?.value || "byedpi",
    proxyHost: body.querySelector('[data-tg-config="proxyHost"]')?.value?.trim() || "",
    proxyPort: Number(body.querySelector('[data-tg-config="proxyPort"]')?.value || 0) || null,
    proxyUsername: body.querySelector('[data-tg-config="proxyUsername"]')?.value?.trim() || "",
    proxyPassword: body.querySelector('[data-tg-config="proxyPassword"]')?.value || "",
  };
  await runTelegramAuthStep(() => invoke("telegram_save_config", { config: config }), card, widgetId);
  telegramRuntime.statusLoaded = false;
  telegramRuntime.chatsLoaded = false;
  telegramRuntime.debugLoaded = false;
  telegramRuntime.config = config;
  telegramRuntime.configLoaded = true;
  renderTelegramWidget(card, widgetId);
}

// Only allow navigable schemes that cannot execute script in the webview.
// textUrl can carry an attacker-chosen URL (javascript:/data:/…); HTML-escaping
// alone does not neutralize those.
const TG_SAFE_HREF_SCHEMES = new Set(["http", "https", "tg", "mailto", "tel"]);

function tgSafeHref(url) {
  const raw = String(url ?? "").trim();
  if (!raw) return "";
  const schemeMatch = raw.match(/^([a-zA-Z][a-zA-Z0-9+.-]*):/);
  if (schemeMatch) {
    if (!TG_SAFE_HREF_SCHEMES.has(schemeMatch[1].toLowerCase())) return "";
    return escapeHtml(raw);
  }
  // Scheme-less entity text (rare): treat as https host/path, not a relative link.
  if (raw.startsWith("//")) return escapeHtml(`https:${raw}`);
  if (/^[\w.-]+\.[a-z]{2,}([/:?#]|$)/i.test(raw)) return escapeHtml(`https://${raw}`);
  return "";
}

function tgLinkHtml(href, inner) {
  if (!href) return `<span class="tg-entity">${inner}</span>`;
  // data-external-url is required: the click handler routes through Tauri open
  // (plain target="_blank" does nothing useful in the WebView).
  return `<a href="${href}" data-external-url="${href}" rel="noopener noreferrer">${inner}</a>`;
}

const TG_MATH_REGEX = /\$\$([\s\S]+?)\$\$|\$([^$\n]+?)\$/g;
let katexLoadPromise = null;

function ensureKatexLoaded() {
  if (window.katex || katexLoadPromise) return katexLoadPromise;
  katexLoadPromise = new Promise((resolve) => {
    if (!document.querySelector('link[data-lazy-katex]')) {
      const css = document.createElement("link");
      css.rel = "stylesheet";
      css.href = "assets/vendor/katex/katex.min.css";
      css.dataset.lazyKatex = "true";
      document.head.appendChild(css);
    }
    const script = document.createElement("script");
    script.src = "assets/vendor/katex/katex.min.js";
    script.dataset.lazyKatex = "true";
    script.onload = () => {
      resolve(window.katex || null);
      const card = getTelegramCard();
      if (card?.classList.contains("expanded")) queueTelegramRender(card, card.dataset.id);
    };
    script.onerror = () => resolve(null);
    document.head.appendChild(script);
  });
  return katexLoadPromise;
}

function tgRenderTextSegment(raw) {
  if (!raw) return "";
  if (raw.indexOf("$") === -1) return escapeHtml(raw);
  if (!window.katex) {
    ensureKatexLoaded();
    return escapeHtml(raw);
  }
  let out = "";
  let last = 0;
  let match;
  TG_MATH_REGEX.lastIndex = 0;
  while ((match = TG_MATH_REGEX.exec(raw))) {
    out += escapeHtml(raw.slice(last, match.index));
    const displayMode = match[1] !== undefined;
    const expr = displayMode ? match[1] : match[2];
    // Inline $...$: require tight delimiters + TeX-like content (avoid "$5, you owe $10").
    if (!displayMode) {
      const looksLikeTex = /[\\^_{}=]/.test(expr);
      const tight = expr.length > 0 && !/^\s/.test(expr) && !/\s$/.test(expr);
      if (!tight || !looksLikeTex) {
        out += escapeHtml(match[0]);
        last = match.index + match[0].length;
        continue;
      }
    }
    // Cap expression length before KaTeX — pathologically long input can hang the WebView.
    if (expr.length > 500) {
      out += escapeHtml(match[0]);
      last = match.index + match[0].length;
      continue;
    }
    try {
      out += window.katex.renderToString(expr, {
        throwOnError: false,
        displayMode,
        maxSize: 20,       // caps any single dimension (em) a construct can expand to
        maxExpand: 200,     // caps macro expansion depth/count
        strict: "ignore",   // don't throw on LaTeX strictness warnings, just degrade
      });
    } catch {
      out += escapeHtml(match[0]);
    }
    last = match.index + match[0].length;
  }
  out += escapeHtml(raw.slice(last));
  return out;
}

// rawInner is the unescaped entity slice (needed for href/scheme checks).
// inner is already HTML-escaped (and may contain nested entity markup).
function tgWrapEntity(entity, inner, rawInner = "") {
  switch (entity.kind) {
    case "bold": return `<b>${inner}</b>`;
    case "italic": return `<i>${inner}</i>`;
    case "underline": return `<u>${inner}</u>`;
    case "strikethrough": return `<s>${inner}</s>`;
    case "code": return `<code>${inner}</code>`;
    case "pre":
    case "preCode": return `<pre>${inner}</pre>`;
    case "spoiler": return `<span class="tg-spoiler">${inner}</span>`;
    case "blockQuote":
    case "expandableBlockQuote": return `<blockquote>${inner}</blockquote>`;
    case "textUrl":
    case "url": {
      // textUrl: entity.url; url: plain entity text (rawInner). Normalize to http(s) only.
      const rawUrl = entity.kind === "textUrl" ? (entity.url || "") : String(rawInner || "");
      const trimmed = rawUrl.trim();
      let normalized = "";
      if (trimmed) {
        if (/^https?:\/\//i.test(trimmed)) {
          normalized = trimmed;
        } else if (/^[a-zA-Z][a-zA-Z0-9+.-]*:/.test(trimmed)) {
          // javascript:, data:, etc. — never render as a navigable link
          normalized = "";
        } else {
          normalized = `https://${trimmed}`;
        }
      }
      if (!normalized) return `<span class="tg-entity">${inner}</span>`;
      // Escape for attributes (rawInner/entity.url are unescaped; keep href + data-external-url consistent)
      const href = escapeHtml(normalized);
      return `<a href="${href}" data-external-url="${href}" rel="noopener noreferrer">${inner}</a>`;
    }
    case "emailAddress":
      return tgLinkHtml(tgSafeHref(`mailto:${String(rawInner || "").replace(/^mailto:/i, "")}`), inner);
    case "phoneNumber":
      return tgLinkHtml(tgSafeHref(`tel:${String(rawInner || "").replace(/^tel:/i, "")}`), inner);
    case "mention":
    case "mentionName":
    case "hashtag":
    case "cashtag":
    case "botCommand":
      return `<span class="tg-entity">${inner}</span>`;
    default:
      return inner;
  }
}

function tgBuildFormattedHtml(text, entities) {
  const raw = text || "";
  if (!entities || !entities.length) return tgRenderTextSegment(raw);
  const sorted = [...entities].sort((a, b) => a.offset - b.offset || b.length - a.length);
  let idx = 0;
  function render(start, end) {
    let out = "";
    let pos = start;
    while (idx < sorted.length && sorted[idx].offset < end) {
      const entity = sorted[idx];
      if (entity.offset < pos) {
        idx++;
        continue;
      }
      out += tgRenderTextSegment(raw.slice(pos, entity.offset));
      idx++;
      const innerEnd = Math.min(entity.offset + entity.length, end);
      const entityRaw = raw.slice(entity.offset, innerEnd);
      const inner = render(entity.offset, innerEnd);
      out += tgWrapEntity(entity, inner, entityRaw);
      pos = innerEnd;
    }
    out += tgRenderTextSegment(raw.slice(pos, end));
    return out;
  }
  return render(0, raw.length);
}

// Shared API/proxy settings fields used by both setup and post-login settings panes.
function renderTelegramSettingsForm(config, debug, tdjsonPath, proxyType, backendAvailable) {
  return `
        <input class="tg-auth-field" data-tg-config="apiId" type="text" placeholder="API ID" value="${escapeHtml(config.apiId)}" />
        <input class="tg-auth-field" data-tg-config="apiHash" type="password" placeholder="${debug?.apiHashConfigured ? "API hash saved" : "API hash"}" value="${escapeHtml(config.apiHash)}" />
        <input class="tg-auth-field" data-tg-config="tdjsonPath" type="text" placeholder="C:\\path\\to\\tdjson.dll" value="${escapeHtml(tdjsonPath)}" />
        <div class="tg-auth-proxy">
          <select class="tg-auth-field" data-tg-config="proxyType">
            <option value="byedpi" ${proxyType === "byedpi" ? "selected" : ""}>ByeDPI desync (managed)</option>
            <option value="none" ${proxyType === "none" ? "selected" : ""}>No proxy</option>
            <option value="socks5" ${proxyType === "socks5" ? "selected" : ""}>SOCKS5 proxy</option>
            <option value="http" ${proxyType === "http" ? "selected" : ""}>HTTP proxy</option>
          </select>
          <input class="tg-auth-field" data-tg-config="proxyHost" type="text" placeholder="Proxy host" value="${escapeHtml(config.proxyHost)}" ${proxyType === "byedpi" ? "disabled" : ""} />
          <input class="tg-auth-field" data-tg-config="proxyPort" type="number" min="1" max="65535" placeholder="Proxy port" value="${escapeHtml(config.proxyPort || "")}" ${proxyType === "byedpi" ? "disabled" : ""} />
          <input class="tg-auth-field" data-tg-config="proxyUsername" type="text" placeholder="Proxy user" value="${escapeHtml(config.proxyUsername)}" ${proxyType === "byedpi" ? "disabled" : ""} />
          <input class="tg-auth-field" data-tg-config="proxyPassword" type="password" placeholder="Proxy password" value="${escapeHtml(config.proxyPassword)}" ${proxyType === "byedpi" ? "disabled" : ""} />
        </div>
        <button type="button" class="tg-auth-action tg-auth-save-config" ${backendAvailable ? "" : "disabled"}>Save settings</button>
        ${backendAvailable ? `<button type="button" class="tg-auth-action tg-auth-restart">Restart TDLib</button>` : ""}
  `;
}

function renderTelegramAccountsSection(status) {
  const accounts = status?.accounts || [];
  const activeId = status?.activeAccountId || "default";
  if (!accounts.length) return "";
  return `
    <div class="tg-settings-accounts">
      <div class="tg-settings-accounts-header">
        <strong>Accounts (${accounts.length})</strong>
        <button type="button" class="tg-settings-account-add-btn">+ Add</button>
      </div>
      <div class="tg-settings-accounts-list">
        ${accounts.map((a) => `
          <div class="tg-settings-account-card ${a.id === activeId ? "active" : ""}" data-acc-id="${escapeHtml(a.id)}">
            <div class="tg-settings-account-info">
              <span class="tg-settings-account-name">${escapeHtml(a.name)}</span>
              ${a.phone ? `<small class="tg-settings-account-sub">${escapeHtml(a.phone)}</small>` : a.username ? `<small class="tg-settings-account-sub">@${escapeHtml(a.username)}</small>` : ""}
            </div>
            <div class="tg-settings-account-actions">
              ${a.id === activeId ? '<span class="tg-settings-account-active-badge">Active</span>' : `
                <button type="button" class="tg-settings-account-switch" data-acc-id="${escapeHtml(a.id)}">Switch</button>
                <button type="button" class="tg-settings-account-remove" data-acc-id="${escapeHtml(a.id)}" aria-label="Remove account">&times;</button>
              `}
            </div>
          </div>
        `).join("")}
      </div>
    </div>
  `;
}

function renderTelegramAuth(status, showSettings = false) {
  const backendAvailable = !!invoke("telegram_status");
  const state = backendAvailable ? (status?.authState || "mock") : "backend_unavailable";
  const hint = backendAvailable
    ? (status?.authHint || "TDLib is not connected yet.")
    : "Live preview can show the widget UI only. Real Telegram login needs the Tauri backend.";
  const config = telegramRuntime.config || {};
  const debug = telegramRuntime.debug;
  const tdjsonPath = config.tdjsonPath || TELEGRAM_DEFAULT_TDJSON_PATH;
  const proxyType = config.proxyType === "vless" ? "byedpi" : (config.proxyType || "byedpi");

  // When the user explicitly clicks the settings button, always show the
  // settings form — even if TDLib is in the middle of the auth flow
  // (WaitPhoneNumber, WaitCode, etc.). Without this, shouldShowTelegramAuth
  // returning true would keep the auth form visible and the settings button
  // would appear broken.
  if (showSettings) {
    return `
      <div class="tg-auth">
        <div class="tg-auth-mark">
          <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
            <path d="M21 4L3 11.5l7 2.4L13 20l3-14-7.2 6.8"/>
          </svg>
        </div>
        <strong>${status?.authorized ? "Telegram Settings" : "Telegram API setup"}</strong>
        <small>${status?.authorized ? "You are logged in." : escapeHtml(hint)}</small>
        ${renderTelegramAccountsSection(status)}
        ${renderTelegramSettingsForm(config, debug, tdjsonPath, proxyType, backendAvailable)}
        ${status?.authorized ? `
          <button type="button" class="tg-auth-action tg-auth-logout" style="background: rgba(var(--status-error-rgb), 0.2); color: var(--status-error); border-color: rgba(var(--status-error-rgb), 0.3);">
            Log Out
          </button>
        ` : ""}
      </div>
    `;
  }
  if (state === "missing_config" || state === "saved_config" || state === "error" || state === "backend_unavailable") {
    return `
      <div class="tg-auth">
        <div class="tg-auth-mark">
          <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
            <path d="M21 4L3 11.5l7 2.4L13 20l3-14-7.2 6.8"/>
          </svg>
        </div>
        <strong>Telegram API setup</strong>
        <small>${escapeHtml(hint)}</small>
        ${debug ? `
          <div class="tg-auth-debug">
            <span>dll ${debug.tdjsonExists ? "ok" : "missing"}</span>
            <span>keys ${debug.apiIdConfigured && debug.apiHashConfigured ? "ok" : "missing"}</span>
            <span>proxy ${debug.proxyConfigured ? "on" : "off"}</span>
            <code>${escapeHtml(debug.tdjsonPath || "tdjson.dll")}</code>
          </div>
        ` : ""}
        ${renderTelegramAccountsSection(status)}
        ${renderTelegramSettingsForm(config, debug, tdjsonPath, proxyType, backendAvailable)}
      </div>
    `;
  }
  if (status?.authorized) {
    return `
      <div class="tg-auth">
        <div class="tg-auth-mark">
          <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
            <path d="M21 4L3 11.5l7 2.4L13 20l3-14-7.2 6.8"/>
          </svg>
        </div>
        <strong>Telegram Settings</strong>
        <small>You are logged in.</small>
        ${renderTelegramAccountsSection(status)}
        ${renderTelegramSettingsForm(config, debug, tdjsonPath, proxyType, backendAvailable)}
        <button type="button" class="tg-auth-action tg-auth-logout" style="background: rgba(var(--status-error-rgb), 0.2); color: var(--status-error); border-color: rgba(var(--status-error-rgb), 0.3);">
          Log Out
        </button>
      </div>
    `;
  }

  const needsPhone = state === "WaitPhoneNumber";
  const needsCode = state === "WaitCode";
  const needsPassword = state === "WaitPassword";
  const needsEmail = state === "WaitEmailAddress";
  const needsEmailCode = state === "WaitEmailCode";
  const needsRegistration = state === "WaitRegistration";
  const needsOtherDevice = state === "WaitOtherDeviceConfirmation";
  const needsEncryptionKey = state === "WaitEncryptionKey";

  const authStateTitles = {
    WaitPhoneNumber: "Enter phone number",
    WaitCode: "Enter login code",
    WaitPassword: "Enter 2FA password",
    WaitEmailAddress: "Enter email address",
    WaitEmailCode: "Enter email code",
    WaitRegistration: "Create your account",
    WaitOtherDeviceConfirmation: "Confirm on another device",
    WaitEncryptionKey: "Setting up local storage…",
  };
  const stateTitle = authStateTitles[state] || state;

  let bodyHtml = "";
  if (needsPhone) {
    bodyHtml = `
      <small>${escapeHtml(hint)}</small>
      <input class="tg-auth-field" data-tg-auth="phone" type="text" placeholder="Phone number" autocomplete="tel" />
      <button type="button" class="tg-auth-action tg-auth-phone">Continue</button>
    `;
  } else if (needsCode) {
    bodyHtml = `
      <small>${escapeHtml(hint)}</small>
      <input class="tg-auth-field" data-tg-auth="code" type="text" placeholder="Login code" inputmode="numeric" autocomplete="one-time-code" />
      <button type="button" class="tg-auth-action tg-auth-code">Continue</button>
    `;
  } else if (needsPassword) {
    bodyHtml = `
      <small>${escapeHtml(hint)}</small>
      <input class="tg-auth-field" data-tg-auth="password" type="password" placeholder="Password" autocomplete="current-password" />
      <button type="button" class="tg-auth-action tg-auth-password">Continue</button>
    `;
  } else if (needsEmail) {
    bodyHtml = `
      <small>${escapeHtml(hint)}</small>
      <input class="tg-auth-field" data-tg-auth="email" type="email" placeholder="Email address" autocomplete="email" />
      <button type="button" class="tg-auth-action tg-auth-email">Continue</button>
    `;
  } else if (needsEmailCode) {
    bodyHtml = `
      <small>${escapeHtml(hint)}</small>
      <input class="tg-auth-field" data-tg-auth="emailCode" type="text" placeholder="Email code" inputmode="numeric" autocomplete="one-time-code" />
      <button type="button" class="tg-auth-action tg-auth-email-code">Continue</button>
    `;
  } else if (needsRegistration) {
    bodyHtml = `
      <small>${escapeHtml(hint)}</small>
      <input class="tg-auth-field" data-tg-auth="firstName" type="text" placeholder="First name" autocomplete="given-name" />
      <input class="tg-auth-field" data-tg-auth="lastName" type="text" placeholder="Last name (optional)" autocomplete="family-name" />
      <button type="button" class="tg-auth-action tg-auth-register">Register</button>
    `;
  } else if (needsOtherDevice) {
    // auth_hint carries the tg:// QR login link when the backend exposes it.
    const link = (hint || "").trim();
    const isLink = /^(tg:\/\/|https?:\/\/)/i.test(link);
    if (isLink) {
      const safeHref = tgSafeHref(link);
      bodyHtml = `
      <small>Scan this link with Telegram on another logged-in device, or open/copy it below.</small>
      <code class="tg-auth-qr-link">${escapeHtml(link)}</code>
      <button type="button" class="tg-auth-action tg-auth-copy-link" data-link="${escapeHtml(link)}">Copy link</button>
      ${safeHref ? `<a class="tg-auth-action" href="${safeHref}" target="_blank" rel="noopener noreferrer">Open link</a>` : ""}
    `;
    } else {
      bodyHtml = `<small>${escapeHtml(link || "Confirm this login on another device.")}</small>`;
    }
  } else if (needsEncryptionKey) {
    // Transitional TDLib state — not stuck; no input or dead "Waiting" button.
    bodyHtml = `<small>Setting up local storage…</small>`;
  } else {
    bodyHtml = `
      <small>${escapeHtml(hint)}</small>
      <button type="button" class="tg-auth-action" disabled>Waiting for TDLib</button>
    `;
  }

  return `
    <div class="tg-auth">
      <div class="tg-auth-mark">
        <svg width="22" height="22" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.8" stroke-linecap="round" stroke-linejoin="round">
          <path d="M21 4L3 11.5l7 2.4L13 20l3-14-7.2 6.8"/>
        </svg>
      </div>
      <strong>${escapeHtml(stateTitle)}</strong>
      ${bodyHtml}
    </div>
  `;
}

function telegramAvatarInner(chat) {
  if (chat.photoPath && convertFileSrc) {
    return `<img class="tg-ava-img" src="${escapeHtml(convertFileSrc(chat.photoPath))}" alt="" loading="lazy" decoding="async" />`;
  }
  return `<span>${escapeHtml(chat.initials)}</span>`;
}

// Avatar downloads are fire-and-forget on the backend (async downloadFile +
// updateFile → telegram-chat-update). Unlike attachments we still allow a few
// automatic retries, but with exponential backoff so a slow/failing file_id
// cannot re-request on every render / 800ms chat poll.
const TG_AVATAR_MAX_ATTEMPTS = 5;
const TG_AVATAR_BASE_COOLDOWN_MS = 2000;
// After a successful "download started" response, wait before re-kicking the
// same file_id while TDLib finishes (backend also dedupes in-flight ids).
const TG_AVATAR_INFLIGHT_COOLDOWN_MS = 8000;

async function handleTelegramAvatarDownload(card, widgetId, chatId, fileId) {
  if (!chatId || !fileId) return;
  const downloads = (telegramRuntime.mediaDownloads ||= new Set());
  const key = String(fileId);
  if (downloads.has(key)) return;

  const retries = (telegramRuntime.avatarRetry ||= new Map());
  const state = retries.get(key) || { attempts: 0, nextRetryAt: 0 };
  const now = Date.now();
  if (state.attempts >= TG_AVATAR_MAX_ATTEMPTS) return;
  if (now < state.nextRetryAt) return;

  downloads.add(key);
  try {
    logFrontend(`telegram avatar: frontend requesting file=${key} chat=${chatId}`);
    const path = await invoke("telegram_download_file", { fileId: Number(fileId) });
    if (path) {
      logFrontend(`telegram avatar: frontend received immediate path for file=${key}`);
      retries.delete(key);
      const list = telegramRuntime.chats || [];
      const index = list.findIndex((chat) => chat.id === chatId);
      if (index !== -1) {
        list[index] = { ...list[index], photoPath: path };
        queueTelegramRender(card, widgetId);
      }
    } else {
      // Backend kicked off (or already has) an async download. Path arrives via
      // chat refresh after updateFile — do not count as a hard failure, but
      // cool down so we don't re-invoke on every paint while in flight.
      state.nextRetryAt = Date.now() + TG_AVATAR_INFLIGHT_COOLDOWN_MS;
      retries.set(key, state);
      logFrontend(`telegram avatar: file=${key} queued; awaiting TDLib updateFile`);
    }
  } catch (e) {
    // Soft backoff (unlike mediaFailed which is permanent until manual retry).
    state.attempts += 1;
    state.nextRetryAt =
      Date.now() + TG_AVATAR_BASE_COOLDOWN_MS * Math.pow(2, Math.min(state.attempts - 1, 4));
    retries.set(key, state);
    logFrontend(`telegram avatar: frontend request failed for file=${key}: ${String(e?.message || e)}`);
  } finally {
    downloads.delete(key);
  }
}

// Coalesces bursts of re-render requests (avatar downloads finishing back to
// back, TDLib update storms) into one scoped update per frame — avoids full
// DOM rebuilds that fight draft/scroll/focus.
let telegramRenderQueued = false;
function queueTelegramRender(card, widgetId) {
  if (telegramRenderQueued) return;
  telegramRenderQueued = true;
  requestAnimationFrame(() => {
    telegramRenderQueued = false;
    const currentCard = getTelegramCard();
    if (currentCard) updateTelegramWidget(currentCard, widgetId);
  });
}

function tgFormatDuration(seconds) {
  if (!seconds || seconds <= 0 || !Number.isFinite(Number(seconds))) return "0:00";
  const s = Math.floor(Number(seconds));
  const hrs = Math.floor(s / 3600);
  const mins = Math.floor((s % 3600) / 60);
  const secs = s % 60;
  const padSecs = String(secs).padStart(2, "0");
  if (hrs > 0) {
    return `${hrs}:${String(mins).padStart(2, "0")}:${padSecs}`;
  }
  return `${mins}:${padSecs}`;
}

function formatTelegramPinnedSnippet(msg) {
  if (!msg) return "Закрепленное сообщение";
  const rawText = msg.text || "";
  const clean = rawText.replace(/\s+/g, " ").trim();
  if (clean) return clean;
  switch (msg.attachmentKind) {
    case "photo": return "📷 Фотография";
    case "video": return msg.attachmentDuration ? `📹 Видео (${tgFormatDuration(msg.attachmentDuration)})` : "📹 Видео";
    case "voice": return msg.attachmentDuration ? `🎤 Голосовое сообщение (${tgFormatDuration(msg.attachmentDuration)})` : "🎤 Голосовое сообщение";
    case "videoNote": return msg.attachmentDuration ? `⭕ Видеосообщение (${tgFormatDuration(msg.attachmentDuration)})` : "⭕ Видеосообщение";
    case "document": return msg.attachmentTitle ? `📎 ${msg.attachmentTitle}` : "📎 Файл";
    case "audio": return msg.attachmentTitle ? `🎵 ${msg.attachmentPerformer ? msg.attachmentPerformer + " — " : ""}${msg.attachmentTitle}` : "🎵 Аудиозапись";
    case "sticker": return msg.sticker ? `🎭 Стикер ${msg.sticker}` : "🎭 Стикер";
    case "animation": return "GIF";
    default:
      if (msg.attachment) return msg.attachment;
      return "Закрепленное сообщение";
  }
}

const tgInFlightPinnedRequests = new Set();
async function ensureTelegramPinnedMessageLoaded(chatId, messageId) {
  const key = `${chatId}:${messageId}`;
  if (tgInFlightPinnedRequests.has(key)) return;
  tgInFlightPinnedRequests.add(key);
  try {
    const msg = await invoke("telegram_get_message", { chatId, messageId });
    if (msg) {
      if (!telegramRuntime.pinnedMessageSnippet) telegramRuntime.pinnedMessageSnippet = new Map();
      telegramRuntime.pinnedMessageSnippet.set(chatId, msg);
      const card = getTelegramCard();
      if (card && card.isConnected) {
        const body = card.querySelector(".widget-body");
        const activeBar = body?.querySelector(`.tg-pinned-bar[data-pinned-id="${CSS.escape(String(messageId))}"]`);
        if (activeBar) {
          const snippetEl = activeBar.querySelector(".tg-pinned-snippet");
          if (snippetEl) snippetEl.textContent = formatTelegramPinnedSnippet(msg);
        }
      }
    }
  } catch (err) {
    // Graceful fallback
  } finally {
    tgInFlightPinnedRequests.delete(key);
  }
}

const tgInFlightChatPinRequests = new Set();
/** How long a failed pin lookup (timeout / client restart) pauses retries. */
const TG_PIN_RETRY_COOLDOWN_MS = 30000;
async function ensureTelegramChatPinnedMessage(chatId, threadId = null, force = false) {
  if (!chatId) return;
  const pinKey = threadId ? `${chatId}:${threadId}` : String(chatId);

  if (!force && telegramRuntime.pinnedMessages?.has(pinKey) && telegramRuntime.pinnedMessages.get(pinKey) !== "0") {
    return;
  }

  // A failed lookup said nothing about whether the pin exists — cool this
  // chat's requests down instead of hammering TDLib on every render. The
  // regular render path retries once the cooldown elapses; explicit pin /
  // unpin actions bypass it (force=true).
  if (!force && (telegramRuntime.pinnedCooldown?.get(pinKey) || 0) > Date.now()) {
    return;
  }

  if (tgInFlightChatPinRequests.has(pinKey)) return;
  tgInFlightChatPinRequests.add(pinKey);

  const epoch = telegramRuntime.chatSwitchEpoch || 0;
  const card = getTelegramCard();
  const widgetId = card?.dataset?.id || "telegram";

  try {
    const res = await invoke("telegram_get_chat_pinned_message", {
      chatId: String(chatId),
      threadId: threadId ? Number(threadId) : null,
    });
    if (telegramRuntime.pinnedCooldown) telegramRuntime.pinnedCooldown.delete(pinKey);

    if (!telegramRuntime.pinnedMessages) telegramRuntime.pinnedMessages = new Map();
    if (!telegramRuntime.pinnedMessageSnippet) telegramRuntime.pinnedMessageSnippet = new Map();

    if (res && res.id) {
      telegramRuntime.pinnedMessages.set(pinKey, String(res.id));
      telegramRuntime.pinnedMessageSnippet.set(pinKey, res);
    } else {
      telegramRuntime.pinnedMessages.set(pinKey, "0");
      telegramRuntime.pinnedMessageSnippet.delete(pinKey);
    }

    const activeId = config.positions?.[widgetId]?.activeChatId;
    const currentThreadId = telegramForumThreadId(chatId);
    if (epoch !== telegramRuntime.chatSwitchEpoch || String(activeId) !== String(chatId) || currentThreadId !== threadId) {
      return;
    }

    const activeCard = getTelegramCard();
    if (!activeCard || !activeCard.isConnected) return;
    if (activeCard._tgTransitioning) {
      activeCard._tgPendingUpdate = true;
    }
    const body = activeCard.querySelector(".widget-body");
    if (!body) return;

    let bar = activeCard.querySelector(".tg-pinned-bar");

    if (res && res.id) {
      const snippet = formatTelegramPinnedSnippet(res);
      if (bar) {
        bar.dataset.pinnedId = String(res.id);
        const snippetEl = bar.querySelector(".tg-pinned-snippet");
        if (snippetEl) snippetEl.textContent = snippet;
      } else {
        const messagesEl = activeCard.querySelector(".tg-thread .tg-messages") || body.querySelector(".tg-messages");
        if (messagesEl) {
          const barHtml = `
            <div class="tg-pinned-bar" data-pinned-id="${escapeHtml(res.id)}">
              <div class="tg-pinned-icon">
                <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
                  <line x1="12" y1="17" x2="12" y2="22"></line>
                  <path d="M5 17h14l-2-7V4h1V2H6v2h1v6l-2 7z"></path>
                </svg>
              </div>
              <div class="tg-pinned-content">
                <span class="tg-pinned-label">Закрепленное сообщение</span>
                <span class="tg-pinned-snippet">${escapeHtml(snippet)}</span>
              </div>
              <button type="button" class="tg-pinned-unpin-btn" data-action="unpin" aria-label="Открепить сообщение">
                <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
                  <line x1="18" y1="6" x2="6" y2="18"></line>
                  <line x1="6" y1="6" x2="18" y2="18"></line>
                </svg>
              </button>
            </div>
          `;
          messagesEl.insertAdjacentHTML("beforebegin", barHtml);
          const newBar = activeCard.querySelector(".tg-pinned-bar");
          const scroller = activeCard.querySelector(".tg-messages");
          const active = (telegramRuntime.chats || []).find((c) => c.id === chatId) || { id: chatId };
          wireTelegramPinnedBar(newBar, activeCard, widgetId, active, scroller);
        }
      }
    } else {
      if (bar) bar.remove();
    }
  } catch (err) {
    // Timeout / client restart — not an answer about the pin's existence.
    // Cache nothing (an absent entry keeps the render path retrying later)
    // and cool the chat down so renders do not refire a request that
    // cannot succeed yet.
    if (!telegramRuntime.pinnedCooldown) telegramRuntime.pinnedCooldown = new Map();
    telegramRuntime.pinnedCooldown.set(pinKey, Date.now() + TG_PIN_RETRY_COOLDOWN_MS);
  } finally {
    tgInFlightChatPinRequests.delete(pinKey);
  }
}

function wireTelegramPinnedBar(pinnedBar, card, widgetId, active, messageScroller) {
  if (!pinnedBar || pinnedBar._tgWired) return;
  pinnedBar._tgWired = true;
  pinnedBar.addEventListener("click", async (e) => {
    const unpinBtn = e.target.closest("[data-action='unpin']");
    if (unpinBtn) {
      e.stopPropagation();
      const pId = pinnedBar.dataset.pinnedId;
      if (active?.id && pId) {
        handleUnpinTelegramMessage(card, widgetId, active.id, pId);
      }
      return;
    }
    const pId = pinnedBar.dataset.pinnedId;
    if (!pId) return;
    jumpToTelegramMessage(card, widgetId, active?.id, pId, pinnedBar);
  });
}

async function handleUnpinTelegramMessage(card, widgetId, chatId, messageId) {
  try {
    const threadId = telegramForumThreadId(chatId);
    const pinKey = threadId ? `${chatId}:${threadId}` : String(chatId);
    await invoke("telegram_unpin_message", { request: { chatId, messageId } });
    if (telegramRuntime.pinnedMessages) telegramRuntime.pinnedMessages.delete(pinKey);
    if (telegramRuntime.pinnedMessageSnippet) telegramRuntime.pinnedMessageSnippet.delete(pinKey);
    const chat = telegramRuntime.chats?.find((c) => c.id === chatId);
    if (chat) chat.pinnedMessageId = null;
    await ensureTelegramChatPinnedMessage(chatId, threadId, true);
  } catch (err) {
    logFrontend("unpin message error: " + err);
  }
}

function tgFormatFileSize(bytes) {
  if (!bytes || bytes <= 0 || !Number.isFinite(Number(bytes))) return "";
  const b = Number(bytes);
  if (b < 1024) return `${b} B`;
  if (b < 1024 * 1024) return `${(b / 1024).toFixed(1)} KB`;
  if (b < 1024 * 1024 * 1024) return `${(b / (1024 * 1024)).toFixed(1)} MB`;
  return `${(b / (1024 * 1024 * 1024)).toFixed(2)} GB`;
}

function tgGetFileExtension(filename) {
  if (!filename) return "FILE";
  const parts = String(filename).split(".");
  if (parts.length < 2) return "FILE";
  const ext = parts.pop().toUpperCase().slice(0, 4);
  return ext || "FILE";
}

function tgBuildWaveformHtml(waveformStr, messageId) {
  const barsCount = 26;
  const bars = [];
  if (waveformStr && typeof waveformStr === "string") {
    try {
      const decoded = atob(waveformStr);
      if (decoded.length > 0) {
        const step = decoded.length / barsCount;
        for (let i = 0; i < barsCount; i++) {
          const idx = Math.min(Math.floor(i * step), decoded.length - 1);
          const val = decoded.charCodeAt(idx) || 0;
          const pct = Math.max(18, Math.min(100, Math.round((val / 31) * 100)));
          bars.push(pct);
        }
      }
    } catch {}
  }
  if (bars.length === 0) {
    let seed = 0;
    const str = String(messageId || "123");
    for (let i = 0; i < str.length; i++) {
      seed = (seed * 31 + str.charCodeAt(i)) % 1000;
    }
    for (let i = 0; i < barsCount; i++) {
      const v = Math.sin((i + seed) * 0.6) * 35 + Math.cos((i * 2 + seed) * 0.4) * 20 + 52;
      bars.push(Math.max(20, Math.min(100, Math.round(v))));
    }
  }
  return bars
    .map((h) => `<span class="tg-voice-bar" style="height: ${h}%;"></span>`)
    .join("");
}

function telegramAttachmentHtml(message, chatId) {
  const kind = message.attachmentKind;
  if (!kind) return "";
  const path = message.attachmentLocalPath;
  const label = escapeHtml(message.attachment || kind);
  const fileId = message.attachmentFileId;
  const durSecs = message.attachmentDuration || 0;
  const durationText = durSecs > 0 ? tgFormatDuration(durSecs) : "";
  const sizeText = message.attachmentSize ? tgFormatFileSize(message.attachmentSize) : "";

  // 1. Photos & Animations (GIFs)
  if (kind === "photo" || kind === "animation") {
    const ratio = message.attachmentWidth && message.attachmentHeight
      ? `style="aspect-ratio: ${Number(message.attachmentWidth)} / ${Number(message.attachmentHeight)};"`
      : "";
    if (path) {
      const src = escapeHtml(convertFileSrc?.(path) || "");
      return kind === "animation"
        ? `<div class="tg-animation-wrap" data-animation-key="${escapeHtml(`${chatId}::${message.id || ""}`)}">
            <video class="tg-media tg-animation" ${ratio} src="${src}" preload="none" loop muted playsinline></video>
            <button type="button" class="tg-animation-toggle" aria-label="Pause animation">
              <svg viewBox="0 0 24 24" width="13" height="13" fill="currentColor"><path d="M6 5h4v14H6zm8 0h4v14h-4z"/></svg>
            </button>
          </div>`
        : `<img class="tg-media" ${ratio} src="${src}" alt="" loading="lazy" decoding="async" fetchpriority="low" />`;
    }
    if (!fileId) {
      return `<button type="button" class="tg-attachment" disabled>${label}</button>`;
    }
    if (telegramRuntime.mediaFailed?.has(String(fileId))) {
      return `<button type="button" class="tg-attachment tg-attachment-load" data-file-id="${fileId}" data-message-id="${escapeHtml(message.id || "")}" data-chat-id="${escapeHtml(chatId)}">${kind === "animation" ? "Повторить GIF" : "Повторить фото"}</button>`;
    }
    return `<span class="tg-media tg-media-pending" ${ratio} data-file-id="${fileId}" data-message-id="${escapeHtml(message.id || "")}" data-chat-id="${escapeHtml(chatId)}"></span>`;
  }

  // 2. Video
  if (kind === "video") {
    const ratio = message.attachmentWidth && message.attachmentHeight
      ? `style="aspect-ratio: ${Number(message.attachmentWidth)} / ${Number(message.attachmentHeight)};"`
      : "";
    const durationBadge = durationText ? `<span class="tg-media-badge">${escapeHtml(durationText)}</span>` : "";
    const sizeBadge = sizeText ? `<span class="tg-media-badge tg-media-badge-right">${escapeHtml(sizeText)}</span>` : "";

    if (path) {
      const src = escapeHtml(convertFileSrc?.(path) || "");
      const totalSecs = Number(message.attachmentDuration || 0);
      return `
        <div class="tg-video-wrap" ${ratio} data-path="${escapeHtml(path)}" data-chat-id="${escapeHtml(chatId)}" data-message-id="${escapeHtml(message.id || "")}">
          <video class="tg-media tg-media-video-player" ${ratio} src="${src}" preload="none" playsinline></video>
          <button type="button" class="tg-video-play-overlay" aria-label="Play">
            <svg class="tg-video-icon-play" viewBox="0 0 24 24" width="22" height="22" fill="currentColor"><path d="M8 5v14l11-7z"/></svg>
            <svg class="tg-video-icon-pause" viewBox="0 0 24 24" width="22" height="22" fill="currentColor" style="display:none;"><path d="M6 19h4V5H6v14zm8-14v14h4V5h-4z"/></svg>
          </button>
          ${durationBadge}
          <div class="tg-video-controls">
            <div class="tg-video-timeline">
              <div class="tg-video-timeline-rail"></div>
              <div class="tg-video-timeline-buffer"></div>
              <div class="tg-video-timeline-progress"></div>
              <div class="tg-video-timeline-thumb"></div>
            </div>
            <div class="tg-video-controls-row">
              <div class="tg-video-ctrls-left">
                <button type="button" class="tg-video-ctrl-btn tg-video-btn-toggle" aria-label="Play/Pause">
                  <svg class="tg-video-icon-play" viewBox="0 0 24 24" width="16" height="16" fill="currentColor"><path d="M8 5v14l11-7z"/></svg>
                  <svg class="tg-video-icon-pause" viewBox="0 0 24 24" width="16" height="16" fill="currentColor" style="display:none;"><path d="M6 19h4V5H6v14zm8-14v14h4V5h-4z"/></svg>
                </button>
                <span class="tg-video-time" data-total="${totalSecs}">
                  <span class="tg-video-time-cur">0:00</span>
                  <span class="tg-video-time-sep">/</span>
                  <span class="tg-video-time-total">${escapeHtml(durationText || "0:00")}</span>
                </span>
              </div>
              <div class="tg-video-ctrls-right">
                <button type="button" class="tg-video-ctrl-btn tg-video-btn-mute" aria-label="Mute/Unmute">
                  <svg class="tg-video-icon-vol-on" viewBox="0 0 24 24" width="16" height="16" fill="currentColor"><path d="M3 9v6h4l5 5V4L7 9H3zm13.5 3c0-1.77-1.02-3.29-2.5-4.03v8.05c1.48-.73 2.5-2.25 2.5-4.02zM14 3.23v2.06c2.89.86 5 3.54 5 6.71s-2.11 5.85-5 6.71v2.06c4.01-.91 7-4.49 7-8.77s-2.99-7.86-7-8.77z"/></svg>
                  <svg class="tg-video-icon-vol-muted" viewBox="0 0 24 24" width="16" height="16" fill="currentColor" style="display:none;"><path d="M16.5 12c0-1.77-1.02-3.29-2.5-4.03v2.21l2.45 2.45c.03-.2.05-.41.05-.63zm2.5 0c0 .94-.2 1.82-.54 2.64l1.51 1.51C20.63 14.91 21 13.5 21 12c0-4.28-2.99-7.86-7-8.77v2.06c2.89.86 5 3.54 5 6.71zM4.27 3L3 4.27 7.73 9H3v6h4l5 5v-6.73l4.25 4.25c-.67.52-1.42.93-2.25 1.18v2.06c1.38-.31 2.63-.95 3.69-1.81L19.73 21 21 19.73l-9-9L4.27 3zM12 4L9.91 6.09 12 8.18V4z"/></svg>
                </button>
                <button type="button" class="tg-video-ctrl-btn tg-video-btn-fullscreen" aria-label="Fullscreen">
                  <svg class="tg-video-icon-fs-enter" viewBox="0 0 24 24" width="15" height="15" fill="currentColor"><path d="M7 14H5v5h5v-2H7v-3zm-2-4h2V7h3V5H5v5zm12 7h-3v2h5v-5h-2v3zM14 5v2h3v3h2V5h-5z"/></svg>
                  <svg class="tg-video-icon-fs-exit" viewBox="0 0 24 24" width="15" height="15" fill="currentColor" style="display:none;"><path d="M5 16h3v3h2v-5H5v2zm3-8H5v2h5V5H8v3zm6 11h2v-3h3v-2h-5v5zm2-11V5h-2v5h5V8h-3z"/></svg>
                </button>
              </div>
            </div>
          </div>
        </div>
      `;
    }
    if (!fileId) {
      return `<button type="button" class="tg-attachment" disabled>${label}</button>`;
    }
    if (telegramRuntime.mediaFailed?.has(String(fileId))) {
      return `<button type="button" class="tg-attachment tg-attachment-load" data-file-id="${fileId}" data-message-id="${escapeHtml(message.id || "")}" data-chat-id="${escapeHtml(chatId)}">Повторить загрузку видео</button>`;
    }
    const thumbImg = message.thumbnailLocalPath
      ? `<img class="tg-media tg-media-thumb" ${ratio} src="${escapeHtml(convertFileSrc?.(message.thumbnailLocalPath) || "")}" alt="" />`
      : "";
    return `
      <div class="tg-video-wrap tg-media-pending" ${ratio} data-file-id="${fileId}" data-message-id="${escapeHtml(message.id || "")}" data-chat-id="${escapeHtml(chatId)}">
        ${thumbImg}
        <button type="button" class="tg-video-download-overlay tg-attachment-download" data-file-id="${fileId}" data-message-id="${escapeHtml(message.id || "")}" data-chat-id="${escapeHtml(chatId)}" aria-label="Скачать видео">
          <svg viewBox="0 0 24 24" width="20" height="20" fill="currentColor"><path d="M19 9h-4V3H9v6H5l7 7 7-7zM5 18v2h14v-2H5z"/></svg>
        </button>
        ${durationBadge}
        ${sizeBadge}
      </div>
    `;
  }

  // 3. Video Note (Round video message / "кружочек")
  if (kind === "videoNote") {
    const durationBadge = durationText ? `<span class="tg-media-badge">${escapeHtml(durationText)}</span>` : "";
    if (path) {
      const src = escapeHtml(convertFileSrc?.(path) || "");
      const totalSecs = Number(message.attachmentDuration || 0);
      return `
        <div class="tg-video-note-wrap" data-path="${escapeHtml(path)}" data-chat-id="${escapeHtml(chatId)}" data-message-id="${escapeHtml(message.id || "")}">
          <div class="tg-video-note-clip">
            <video class="tg-video-note-player" src="${src}" preload="none" loop muted playsinline></video>
          </div>
          <svg class="tg-video-note-ring" viewBox="0 0 170 170">
            <circle class="ring-bg" cx="85" cy="85" r="83.5"></circle>
            <circle class="ring-progress" cx="85" cy="85" r="83.5"></circle>
          </svg>
          <button type="button" class="tg-video-note-overlay" aria-label="Play with sound">
            <svg class="icon-play" viewBox="0 0 24 24" width="20" height="20" fill="currentColor"><path d="M8 5v14l11-7z"/></svg>
            <svg class="icon-pause" viewBox="0 0 24 24" width="20" height="20" fill="currentColor" style="display:none;"><path d="M6 19h4V5H6v14zm8-14v14h4V5h-4z"/></svg>
            <svg class="icon-replay" viewBox="0 0 24 24" width="20" height="20" fill="currentColor" style="display:none;"><path d="M12 5V1L7 6l5 5V7c3.31 0 6 2.69 6 6s-2.69 6-6 6-6-2.69-6-6H4c0 4.42 3.58 8 8 8s8-3.58 8-8-3.58-8-8-8z"/></svg>
          </button>
          ${durationBadge}
        </div>
      `;
    }
    if (!fileId) {
      return `<button type="button" class="tg-attachment" disabled>${label}</button>`;
    }
    const noteThumb = message.thumbnailLocalPath
      ? `<img class="tg-video-note-thumb" src="${escapeHtml(convertFileSrc?.(message.thumbnailLocalPath) || "")}" alt="" />`
      : "";
    return `
      <div class="tg-video-note-wrap tg-media-pending" data-file-id="${fileId}" data-message-id="${escapeHtml(message.id || "")}" data-chat-id="${escapeHtml(chatId)}">
        ${noteThumb}
        <button type="button" class="tg-video-note-overlay tg-attachment-download" data-file-id="${fileId}" data-message-id="${escapeHtml(message.id || "")}" data-chat-id="${escapeHtml(chatId)}" aria-label="Скачать кружочек">
          <svg viewBox="0 0 24 24" width="18" height="18" fill="currentColor"><path d="M19 9h-4V3H9v6H5l7 7 7-7zM5 18v2h14v-2H5z"/></svg>
        </button>
        ${durationBadge}
      </div>
    `;
  }

  // 4. Voice Message
  if (kind === "voice") {
    if (path) {
      const src = escapeHtml(convertFileSrc?.(path) || "");
      return `
        <div class="tg-voice-card ready" data-path="${escapeHtml(path)}" data-chat-id="${escapeHtml(chatId)}" data-message-id="${escapeHtml(message.id || "")}">
          <button type="button" class="tg-voice-play-btn tg-voice-toggle-btn" aria-label="Воспроизвести">
            <svg class="tg-icon-play" viewBox="0 0 24 24" width="16" height="16" fill="currentColor"><path d="M8 5v14l11-7z"/></svg>
            <svg class="tg-icon-pause" viewBox="0 0 24 24" width="16" height="16" fill="currentColor"><path d="M6 19h4V5H6v14zm8-14v14h4V5h-4z"/></svg>
          </button>
          <div class="tg-voice-body">
            <div class="tg-voice-waveform">
              ${tgBuildWaveformHtml(message.attachmentWaveform, message.id)}
              <div class="tg-voice-progress-fill" style="width: 0%;"></div>
            </div>
            <div class="tg-voice-meta">
              <span class="tg-voice-duration" data-total="${durSecs}">${durationText || "0:00"}</span>
            </div>
          </div>
          <audio class="tg-voice-audio-el" src="${src}" preload="metadata"></audio>
        </div>
      `;
    }
    if (!fileId) {
      return `<button type="button" class="tg-attachment" disabled>${label}</button>`;
    }
    return `
      <div class="tg-voice-card pending tg-media-pending" data-file-id="${fileId}" data-message-id="${escapeHtml(message.id || "")}" data-chat-id="${escapeHtml(chatId)}">
        <button type="button" class="tg-voice-play-btn tg-attachment-download" data-file-id="${fileId}" data-message-id="${escapeHtml(message.id || "")}" data-chat-id="${escapeHtml(chatId)}" aria-label="Скачать голосовое">
          <svg class="tg-icon-play" viewBox="0 0 24 24" width="16" height="16" fill="currentColor"><path d="M8 5v14l11-7z"/></svg>
        </button>
        <div class="tg-voice-body">
          <div class="tg-voice-waveform">
            ${tgBuildWaveformHtml(message.attachmentWaveform, message.id)}
            <div class="tg-voice-progress-fill" style="width: 0%;"></div>
          </div>
          <div class="tg-voice-meta">
            <span class="tg-voice-duration">${durationText || "0:00"}</span>
          </div>
        </div>
      </div>
    `;
  }

  // 5. Audio / Music Track
  if (kind === "audio") {
    const title = message.attachmentTitle || message.attachment || "Аудио";
    const performer = message.attachmentPerformer || (sizeText ? sizeText : "Аудиозапись");
    if (path) {
      const src = escapeHtml(convertFileSrc?.(path) || "");
      return `
        <div class="tg-audio-card ready" data-path="${escapeHtml(path)}" data-chat-id="${escapeHtml(chatId)}" data-message-id="${escapeHtml(message.id || "")}">
          <button type="button" class="tg-audio-play-btn tg-audio-toggle-btn" aria-label="Воспроизвести">
            <svg class="tg-icon-play" viewBox="0 0 24 24" width="18" height="18" fill="currentColor"><path d="M8 5v14l11-7z"/></svg>
            <svg class="tg-icon-pause" viewBox="0 0 24 24" width="18" height="18" fill="currentColor"><path d="M6 19h4V5H6v14zm8-14v14h4V5h-4z"/></svg>
          </button>
          <div class="tg-audio-body">
            <div class="tg-audio-title">${escapeHtml(title)}</div>
            <div class="tg-audio-sub">
              <span class="tg-audio-performer">${escapeHtml(performer)}</span>
              <span class="tg-audio-duration" data-total="${durSecs}">${durationText || ""}</span>
            </div>
            <div class="tg-audio-track">
              <div class="tg-audio-track-fill" style="width: 0%;"></div>
            </div>
          </div>
          <audio class="tg-audio-el" src="${src}" preload="metadata"></audio>
        </div>
      `;
    }
    if (!fileId) {
      return `<button type="button" class="tg-attachment" disabled>${label}</button>`;
    }
    return `
      <div class="tg-audio-card pending tg-media-pending" data-file-id="${fileId}" data-message-id="${escapeHtml(message.id || "")}" data-chat-id="${escapeHtml(chatId)}">
        <button type="button" class="tg-audio-play-btn tg-attachment-download" data-file-id="${fileId}" data-message-id="${escapeHtml(message.id || "")}" data-chat-id="${escapeHtml(chatId)}" aria-label="Скачать аудио">
          <svg class="tg-icon-play" viewBox="0 0 24 24" width="18" height="18" fill="currentColor"><path d="M8 5v14l11-7z"/></svg>
        </button>
        <div class="tg-audio-body">
          <div class="tg-audio-title">${escapeHtml(title)}</div>
          <div class="tg-audio-sub">
            <span class="tg-audio-performer">${escapeHtml(performer)}</span>
            <span class="tg-audio-duration">${durationText || ""}</span>
          </div>
          <div class="tg-audio-track">
            <div class="tg-audio-track-fill" style="width: 0%;"></div>
          </div>
        </div>
      </div>
    `;
  }

  // 6. Native Document / Generic File with Explorer SVG Icons
  const extTag = tgGetFileExtension(label);
  const extLower = extTag.toLowerCase();
  const iconName = getFileIconName(label);
  const iconSrc = `assets/icons/file-types/${iconName}`;
  const typeDesc = getFileTypeName(label);

  if (path) {
    return `
      <div class="tg-file-card ready tg-attachment-open" data-path="${escapeHtml(path)}" data-chat-id="${escapeHtml(chatId)}" data-message-id="${escapeHtml(message.id || "")}">
        <div class="tg-file-icon-box tg-file-${extLower}">
          <img class="tg-file-svg-icon" src="${iconSrc}" alt="" loading="lazy" />
          <div class="tg-file-icon-action">
            <svg viewBox="0 0 24 24" width="16" height="16" fill="none" stroke="currentColor" stroke-width="2"><path d="M14 2H6a2 2 0 00-2 2v16a2 2 0 002 2h12a2 2 0 002-2V8z"/><polyline points="14 2 14 8 20 8"/></svg>
          </div>
        </div>
        <div class="tg-file-body">
          <div class="tg-file-name">${label}</div>
          <div class="tg-file-meta">
            <span>${sizeText || typeDesc}</span>
            <span class="tg-file-badge-ok">· Скачано</span>
          </div>
        </div>
      </div>
    `;
  }
  if (fileId) {
    return `
      <div class="tg-file-card pending tg-attachment-download" data-file-id="${fileId}" data-message-id="${escapeHtml(message.id || "")}" data-chat-id="${escapeHtml(chatId)}">
        <div class="tg-file-icon-box tg-file-${extLower}">
          <img class="tg-file-svg-icon" src="${iconSrc}" alt="" loading="lazy" />
          <div class="tg-file-icon-action">
            <svg viewBox="0 0 24 24" width="16" height="16" fill="currentColor"><path d="M19 9h-4V3H9v6H5l7 7 7-7zM5 18v2h14v-2H5z"/></svg>
          </div>
        </div>
        <div class="tg-file-body">
          <div class="tg-file-name">${label}</div>
          <div class="tg-file-meta">
            <span>${sizeText || typeDesc}</span>
            <span class="tg-file-badge-down">· Нажмите для загрузки</span>
          </div>
        </div>
      </div>
    `;
  }
  return `<button type="button" class="tg-attachment" disabled>${label}</button>`;
}

// Telegram renders a message made of only emoji (up to 3) large and without a
// bubble. Detect that: strip every emoji code point plus its modifiers (skin
// tones, ZWJ joins, variation selectors, regional-indicator flags) and see if
// anything is left. Digits/`*`/`#` are deliberately kept so "123" stays a normal
// message and isn't mistaken for keycap emoji.
function tgIsEmojiOnly(text) {
  const trimmed = (text || "").trim();
  if (!trimmed) return false;
  let stripped;
  try {
    stripped = trimmed
      .replace(/[\u{1F1E6}-\u{1F1FF}]/gu, "")
      .replace(/[\u{1F3FB}-\u{1F3FF}]/gu, "")
      .replace(/[‍️︎⃣]/gu, "")
      .replace(/\p{Extended_Pictographic}/gu, "")
      .replace(/\s/g, "");
  } catch {
    return false; // Unicode property escapes unsupported — treat as normal text.
  }
  if (stripped.length > 0) return false;
  // Require at least one real emoji so a stray ZWJ/space isn't treated as one.
  // Regional-indicator flags aren't Extended_Pictographic, so check them too.
  return /\p{Extended_Pictographic}/u.test(trimmed) || /[\u{1F1E6}-\u{1F1FF}]/u.test(trimmed);
}

function tgGraphemeCount(text) {
  const trimmed = (text || "").trim();
  if (typeof Intl !== "undefined" && Intl.Segmenter) {
    let count = 0;
    for (const _ of new Intl.Segmenter(undefined, { granularity: "grapheme" }).segment(trimmed)) count++;
    return count;
  }
  return [...trimmed].length;
}

function formatLastSeenRelative(secondsAgo) {
  if (secondsAgo < 60) return "был(а) только что";
  const mins = Math.floor(secondsAgo / 60);
  if (mins < 60) return `был(а) ${mins} мин. назад`;
  const hours = Math.floor(mins / 60);
  if (hours < 24) return `был(а) ${hours} ч. назад`;
  const days = Math.floor(hours / 24);
  if (days === 1) return "был(а) вчера";
  if (days < 7) return `был(а) ${days} дн. назад`;
  return "был(а) давно";
}

function formatTelegramChatStatus(chat) {
  if (!chat) return { text: "", online: false };
  if (chat.kind === "bot") return { text: "бот", online: false };
  if (chat.kind === "channel") {
    if (chat.memberCount) {
      return { text: `${chat.memberCount.toLocaleString()} подписчиков`, online: false };
    }
    return { text: "канал", online: false };
  }
  if (chat.kind === "group") {
    if (chat.memberCount) {
      return { text: `${chat.memberCount.toLocaleString()} участников`, online: false };
    }
    return { text: "группа", online: false };
  }

  // Private or secret chat:
  const statusStr = chat.status || (telegramRuntime.userStatuses && chat.userId ? telegramRuntime.userStatuses.get(String(chat.userId)) : null);
  if (!statusStr) return { text: "был(а) недавно", online: false };

  if (statusStr === "online") return { text: "в сети", online: true };
  if (statusStr === "recently") return { text: "был(а) недавно", online: false };
  if (statusStr === "last_week") return { text: "был(а) на этой неделе", online: false };
  if (statusStr === "last_month") return { text: "был(а) в этом месяце", online: false };
  if (statusStr.startsWith("was_online:")) {
    const ts = parseInt(statusStr.split(":")[1], 10);
    if (ts) {
      const nowSec = Math.floor(Date.now() / 1000);
      const diff = Math.max(0, nowSec - ts);
      return { text: formatLastSeenRelative(diff), online: false };
    }
  }
  return { text: statusStr, online: false };
}

function renderTelegramThread(chat, messages) {
  if (!chat) {
    const isAuth = shouldShowTelegramAuth(telegramRuntime.status);
    if (isAuth) return renderTelegramAuth(telegramRuntime.status);
    const pendingFile = telegramRuntime.pendingExternalFile;
    const pendingFileName = pendingFile ? pendingFile.split(/[/\\]/).pop() : "";
    const pendingFileHtml = pendingFile ? `
      <div class="tg-pending-file" data-path="${escapeHtml(pendingFile)}">
        <span class="tg-pending-file-name" aria-label="${escapeHtml(pendingFile)}">${escapeHtml(pendingFileName)}</span>
        <button type="button" class="tg-pending-file-remove" aria-label="Remove attached file">×</button>
      </div>
    ` : "";
    return `
      <div class="tg-thread-head">
        <span class="tg-thread-avatar">?</span>
        <div class="tg-thread-head-info">
          <strong>Telegram</strong>
          <small class="tg-status-line">Select a chat</small>
        </div>
      </div>
      <div class="tg-messages" data-chat-id="">
        <div class="tg-state">${telegramRuntime.loadingChats ? "Loading chats..." : "Select a chat to view messages"}</div>
      </div>
      <div class="tg-composer">
        <div class="tg-mention-pop"></div>
        ${pendingFileHtml}
        <div class="tg-composer-row">
          <button type="button" class="tg-attach" aria-label="Attach" disabled>
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round">
              <path d="M21.4 11.6l-8.7 8.7a6 6 0 01-8.5-8.5l9.2-9.2a4 4 0 015.7 5.7l-9.2 9.2a2 2 0 01-2.8-2.8l8.7-8.7"/>
            </svg>
          </button>
          <textarea class="tg-input" rows="1" placeholder="${pendingFile ? "Add a caption..." : "Message"}" disabled></textarea>
          <button type="button" class="tg-send" aria-label="Send" disabled>
            <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round">
              <path d="M22 2L11 13"/>
              <path d="M22 2l-7 20-4-9-9-4 20-7z"/>
            </svg>
          </button>
        </div>
      </div>
    `;
  }
  const pendingFile = telegramRuntime.pendingExternalFile;
  const pendingFileName = pendingFile ? pendingFile.split(/[/\\]/).pop() : "";
  const pendingFileHtml = pendingFile ? `
    <div class="tg-pending-file" data-path="${escapeHtml(pendingFile)}">
      <span class="tg-pending-file-name" aria-label="${escapeHtml(pendingFile)}">${escapeHtml(pendingFileName)}</span>
      <button type="button" class="tg-pending-file-remove" aria-label="Remove attached file">×</button>
    </div>
  ` : "";

  const replyMsg = chat?.id && telegramRuntime.replyMessage ? telegramRuntime.replyMessage.get(chat.id) : null;
  const replyBarHtml = replyMsg ? `
    <div class="tg-reply-bar">
      <div class="tg-reply-bar-content">
        <span class="tg-reply-bar-author">${escapeHtml(replyMsg.from || "Message")}</span>
        <span class="tg-reply-bar-text">${escapeHtml(replyMsg.text || "...")}</span>
      </div>
      <button type="button" class="tg-reply-bar-close" aria-label="Cancel reply">×</button>
    </div>
  ` : "";

  const editMsg = chat?.id && telegramRuntime.editMessage ? telegramRuntime.editMessage.get(chat.id) : null;
  const editBarHtml = editMsg ? `
    <div class="tg-reply-bar tg-edit-bar">
      <div class="tg-reply-bar-content">
        <span class="tg-reply-bar-author">Editing</span>
        <span class="tg-reply-bar-text">${escapeHtml(editMsg.text || "...")}</span>
      </div>
      <button type="button" class="tg-edit-bar-close" aria-label="Cancel edit">×</button>
    </div>
  ` : "";

  const chatStatus = formatTelegramChatStatus(chat);
  const statusHtml = `
    <small class="tg-status-line ${chatStatus.online ? 'tg-status-online' : ''}">
      ${chatStatus.online ? '<span class="tg-online-dot"></span>' : ''}
      ${escapeHtml(chatStatus.text)}
    </small>
  `;

  const selectedTopic = (telegramRuntime.forumTopics.get(chat.id) || [])
    .find((topic) => String(topic.id) === String(telegramForumThreadId(chat.id)));
  const forumBarHtml = chat.isForum ? `
    <div class="tg-forum-topics-bar" data-chat-id="${escapeHtml(chat.id)}">
      ${telegramForumThreadId(chat.id) ? `
        <button type="button" class="tg-topic-back-btn" data-action="back-to-topics">
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round"><path d="M15 18l-6-6 6-6"/></svg>
          <span>Topics</span>
        </button>
        <span class="tg-topics-chat-title">${escapeHtml(selectedTopic?.title || 'Topic')}</span>
      ` : ""}
    </div>
  ` : "";

  // Forum chat with no topic selected: keep the chat header but replace the
  // messages with a hint instead of showing the general chat (the topics list
  // lives in its own column next to the thread).
  const showTopicHint = chat.isForum && !telegramForumThreadId(chat.id);
  const messagesInner = showTopicHint
    ? `<div class="tg-state">Select a topic</div>`
    : buildMessagesInnerHtml(chat, messages);

  const threadId = telegramForumThreadId(chat.id);
  const pinKey = threadId ? `${chat.id}:${threadId}` : String(chat.id);

  let pinnedId = (chat?.pinnedMessageId && String(chat.pinnedMessageId) !== "0")
    ? String(chat.pinnedMessageId)
    : (telegramRuntime.pinnedMessages?.get(pinKey));

  let pinnedMsg = null;
  if (!pinnedId && Array.isArray(messages)) {
    const optimisticPin = messages.slice().reverse().find((m) => m.isPinned === true || m.is_pinned === true);
    if (optimisticPin) {
      pinnedId = String(optimisticPin.id);
      pinnedMsg = optimisticPin;
      if (!telegramRuntime.pinnedMessages) telegramRuntime.pinnedMessages = new Map();
      telegramRuntime.pinnedMessages.set(pinKey, pinnedId);
      if (!telegramRuntime.pinnedMessageSnippet) telegramRuntime.pinnedMessageSnippet = new Map();
      telegramRuntime.pinnedMessageSnippet.set(pinKey, optimisticPin);
    }
  }

  if (chat?.id && !showTopicHint) {
    ensureTelegramChatPinnedMessage(chat.id, threadId);
  }

  let pinnedBarHtml = "";
  if (pinnedId && pinnedId !== "0" && !showTopicHint) {
    if (!pinnedMsg && Array.isArray(messages)) {
      pinnedMsg = messages.find((m) => String(m.id) === String(pinnedId));
    }
    if (!pinnedMsg && chat?.id) {
      pinnedMsg = telegramRuntime.pinnedMessageSnippet?.get(pinKey)
        || telegramRuntime.messages?.get(telegramMessageCacheKey(chat.id, threadId))?.find((m) => String(m.id) === String(pinnedId))
        || null;
      if (!pinnedMsg) {
        ensureTelegramPinnedMessageLoaded(chat.id, pinnedId);
      }
    }
    const snippet = formatTelegramPinnedSnippet(pinnedMsg);
    pinnedBarHtml = `
      <div class="tg-pinned-bar" data-pinned-id="${escapeHtml(pinnedId)}">
        <div class="tg-pinned-icon">
          <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.2" stroke-linecap="round" stroke-linejoin="round">
            <line x1="12" y1="17" x2="12" y2="22"></line>
            <path d="M5 17h14l-2-7V4h1V2H6v2h1v6l-2 7z"></path>
          </svg>
        </div>
        <div class="tg-pinned-content">
          <span class="tg-pinned-label">Закрепленное сообщение</span>
          <span class="tg-pinned-snippet">${escapeHtml(snippet)}</span>
        </div>
        <button type="button" class="tg-pinned-unpin-btn" data-action="unpin" aria-label="Открепить сообщение">
          <svg width="10" height="10" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
            <line x1="18" y1="6" x2="6" y2="18"></line>
            <line x1="6" y1="6" x2="18" y2="18"></line>
          </svg>
        </button>
      </div>
    `;
  }

  return `
    <div class="tg-thread-head">
      <span class="tg-thread-avatar" data-initials="${escapeHtml(chat.initials)}">${telegramAvatarInner(chat)}</span>
      <div class="tg-thread-head-info">
        <strong>${escapeHtml(chat.title)}</strong>
        ${statusHtml}
      </div>
    </div>
    ${forumBarHtml}
    ${pinnedBarHtml}
    <div class="tg-messages">
      ${messagesInner}
    </div>
    <button type="button" class="tg-scroll-down-btn" aria-label="Перейти к новым сообщениям">
      <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
        <line x1="12" y1="5" x2="12" y2="19"></line>
        <polyline points="19 12 12 19 5 12"></polyline>
      </svg>
      <span class="tg-scroll-down-badge" style="display:none">0</span>
    </button>
    <div class="tg-composer">
      <div class="tg-mention-pop"></div>
      ${replyBarHtml}
      ${editBarHtml}
      ${pendingFileHtml}
      <div class="tg-composer-row">
        <button type="button" class="tg-attach" aria-label="Attach">
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round">
            <path d="M21.4 11.6l-8.7 8.7a6 6 0 01-8.5-8.5l9.2-9.2a4 4 0 015.7 5.7l-9.2 9.2a2 2 0 01-2.8-2.8l8.7-8.7"/>
          </svg>
        </button>
        <textarea class="tg-input" rows="1" placeholder="${pendingFile ? "Add a caption..." : editMsg ? "Edit message..." : "Message"}"></textarea>
        <button type="button" class="tg-send" aria-label="Send">
          <svg width="15" height="15" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.1" stroke-linecap="round" stroke-linejoin="round">
            <path d="M22 2L11 13"/>
            <path d="M22 2l-7 20-4-9-9-4 20-7z"/>
          </svg>
        </button>
      </div>
    </div>
  `;
}

// Deterministic YouTube-style skeleton placeholders. Never carried data-* / a
// .tg-chat-row class / <button> — otherwise wireTelegramContextMenus and the
// in-place chat-row patchers would treat them as real chats. Output is a pure
// static string so the .tg-chat-list cache comparison never flips.
function telegramSkeletonRows(count) {
  const widths = [58, 34, 66, 30, 52, 40, 71, 28, 62, 38, 49, 44];
  let out = "";
  for (let i = 0; i < count; i++) {
    const w = widths[i % widths.length];
    out += `<div class="tg-skeleton-row" role="presentation" aria-hidden="true">
        <span class="tg-skeleton-avatar"></span>
        <span class="tg-skeleton-lines">
          <span class="tg-skeleton-line" style="width: ${w}%"></span>
          <span class="tg-skeleton-line short" style="width: ${Math.max(24, Math.round(w * 0.55))}%"></span>
        </span>
      </div>`;
  }
  return out;
}

function renderTelegramDock(card, widgetId) {
  const body = card.querySelector(".widget-body");
  if (!body) return;
  const chats = telegramVisibleChats();
  const activeId = config.positions[widgetId]?.activeChatId || chats[0]?.id;
  const railHtml = buildRailHtml(chats, activeId);
  card._tgSectionCache = { rail: railHtml, list: null, thread: null, topics: null, topicsChatId: null };
  body.innerHTML = `<div class="tg-shell">
    <button type="button" class="tg-rail-toggle" aria-label="Telegram widget"><span></span></button>
    <div class="tg-rail">${railHtml}</div>
  </div>`;
  wireTelegramRailSection(card, widgetId, body);
  kickTelegramAvatarDownloads(card, widgetId, chats);
  body.querySelector(".tg-rail-toggle")?.addEventListener("click", (event) => {
    event.stopPropagation();
    if (!editMode) toggleTelegramWidget(card, widgetId);
  });
}

// ── Avatar continuity (dock rail <-> expanded chat list) ─────────────────────
// Every avatar visible in both states flies between its two positions in
// lockstep with the surface morph. Flights are ghosts in the portal: the card
// clips its own content, so a real avatar could not leave it mid-morph.

/** chatId -> {el, rect} for avatars inside their scroller's visible area. */
function telegramAvatarRects(card, scrollerSel, avatarSel) {
  const out = new Map();
  const scroller = card.querySelector(scrollerSel);
  if (!scroller) return out;
  const view = scroller.getBoundingClientRect();
  for (const el of scroller.querySelectorAll(avatarSel)) {
    const host = el.closest('[data-chat-id]');
    const rect = el.getBoundingClientRect();
    if (!host || rect.width === 0 || rect.bottom <= view.top || rect.top >= view.bottom) continue;
    out.set(host.dataset.chatId, { el, rect });
  }
  return out;
}

function cancelTelegramAvatarFlight(card) {
  card._tgAvatarFlight?.finish();
  card._tgAvatarFlight = null;
}

/**
 * Fly avatars from `from` to `to` positions (Map chatId -> {el, rect}).
 * The flight ends when both its timeline and `settle()` (the surface's own
 * finalize) have run, so ghosts never vanish before the real avatars paint.
 */
function flyTelegramAvatars(card, from, to, open, duration) {
  cancelTelegramAvatarFlight(card);
  if (prefersReducedMotion()) return;
  const pairs = [...from].filter(([id]) => to.has(id));
  if (!pairs.length) return;
  const layer = document.createElement('div');
  layer.className = 'tg-avatar-flight';
  layer.setAttribute('aria-hidden', 'true');
  surfaceManager.getPortal().appendChild(layer);
  const hidden = [];
  const hide = el => { if (el?.isConnected) { el.style.visibility = 'hidden'; hidden.push(el); } };
  let pending = 2;
  const flight = {
    target: null,
    finish() {
      timeline.cleanup();
      layer.remove();
      for (const el of hidden) el.style.visibility = '';
      if (card._tgAvatarFlight === flight) card._tgAvatarFlight = null;
    },
    settle() { if (--pending === 0) flight.finish(); },
  };
  const timeline = motionTimeline({ duration, durationIsScaled: true,
    easing: open ? EASINGS.expoOut : EASINGS.surfaceExit,
    onFinish: () => flight.settle(), onCancel: () => flight.finish() });
  for (const [id, a] of pairs) {
    const b = to.get(id);
    const ghost = a.el.cloneNode(true);
    ghost.style.cssText = `position:fixed;left:${a.rect.left}px;top:${a.rect.top}px;width:${a.rect.width}px;height:${a.rect.height}px;margin:0;transform-origin:0 0;`;
    layer.appendChild(ghost);
    const scale = b.rect.width / a.rect.width;
    timeline.add(ghost, [
      { transform: 'translate(0px, 0px) scale(1)' },
      { transform: `translate(${b.rect.left - a.rect.left}px, ${b.rect.top - a.rect.top}px) scale(${scale})` },
    ]);
    hide(a.el);
    hide(b.el);
  }
  card._tgAvatarFlight = flight;
  timeline.play();
}

function activateTelegramExpandedRender(card, widgetId) {
  if (!card.isConnected || !card.classList.contains('expanded') ||
      surfaceManager.getRecord(card)?.state !== 'open') return;
  card._tgSurfaceTransitioning = false;
  card._tgPreparing = false;
  card.querySelector('.widget-body').inert = false;
  const activate = card._tgActivatePrepared;
  card._tgActivatePrepared = null;
  activate?.();
  card._tgPendingUpdate = false;
  updateTelegramWidget(card, widgetId);
}

function toggleTelegramWidget(card, widgetId) {
  // Edit/move mode is for layout only — never open the full chat panel.
  if (editMode) return;

  const activeSurface = surfaceManager.getRecord(card);
  if (activeSurface?.state === "closing") {
    card._tgSurfaceTransitioning = true;
    card._tgContentGeneration = (card._tgContentGeneration || 0) + 1;
    const pos = config.positions[widgetId] || (config.positions[widgetId] = {});
    pos.expanded = true;
    card.classList.add("expanded", "surface-washed");
    surfaceManager.openSurface(card);
    if (pos.activeChatId) {
      openTelegramChatSession(pos.activeChatId);
    }
    return;
  }

  // Any surface that is opening or open folds on a repeated click.
  if (card.classList.contains("expanded") || surfaceManager.getRecord(card)) {
    collapseTelegramWidget(card, widgetId);
    return;
  }

  // Collapse other expanded desktop widgets cleanly
  document.querySelectorAll(".widget-card.expanded").forEach((other) => {
    if (other === card) return;
    if (other.classList.contains("telegram-widget")) {
      collapseTelegramWidget(other, other.dataset.id);
      return;
    }
    const otherId = other.dataset.id;
    const oPos = config.positions[otherId] || { x: 28, y: 28 };
    const otherSurface = surfaceManager.getRecord(other);
    if (otherSurface && otherSurface.state !== "closed") {
      if (oPos) oPos.expanded = false;
      surfaceManager.closeSurface(other);
      return;
    }
    other.style.left = (oPos.x ?? 28) + "px";
    other.style.top = (oPos.y ?? 28) + "px";
    other.style.width = (oPos.w || 340) + "px";
    other.style.height = (oPos.h || 64) + "px";
    other.classList.remove("expanded");
    if (oPos) oPos.expanded = false;
    const dur = typeof getDuration === "function" ? getDuration("motion") : 320;
    if (other._collapseTimer) clearTimeout(other._collapseTimer);
    other._collapseTimer = setTimeout(() => {
      other._collapseTimer = null;
      other.style.zIndex = "10";
      const b = other.querySelector(".widget-body");
      if (b && !other.classList.contains("expanded")) b.innerHTML = "";
    }, dur);
  });

  const pos = config.positions[widgetId] || (config.positions[widgetId] = {});
  const canvasW = widgetsCanvas.clientWidth || window.innerWidth || 1920;
  const canvasH = widgetsCanvas.clientHeight || window.innerHeight || 900;
  const margin = 10;

  // Anchor to the dock's live geometry, then grow into free space
  const rect = card.getBoundingClientRect();
  const canvasRect = widgetsCanvas.getBoundingClientRect();
  const dockX = rect.left - canvasRect.left;
  const dockY = rect.top - canvasRect.top;
  const dockW = rect.width || pos.w || 56;
  const dockH = rect.height || pos.h || 520;

  const targetW = Math.min(560, Math.max(440, canvasW - margin * 2 - 40));
  // Keep Telegram on the dock's vertical lane. The reveal grows only sideways.
  const targetH = dockH;
  const clampedW = Math.min(targetW, Math.max(280, canvasW - margin * 2));
  const maxH = Math.max(280, canvasH - margin * 2);
  const clampedH = Math.min(targetH, maxH);

  // Right half of the screen → open left; left half → open right.
  const dockCenterX = dockX + dockW / 2;
  const expandRight = dockCenterX < canvasW / 2;

  let left = expandRight ? dockX : dockX + dockW - clampedW;
  let top = dockY;
  if (top + clampedH > canvasH - margin) {
    top = Math.max(margin, canvasH - clampedH - margin);
  }
  left = Math.max(margin, Math.min(left, canvasW - clampedW - margin));
  top = Math.max(margin, Math.min(top, canvasH - clampedH - margin));
  const horizontalOnly = Math.abs(clampedH - dockH) < 1 && Math.abs(top - dockY) < 1;

  card.classList.toggle("expand-right", expandRight);
  card.classList.toggle("expand-left", !expandRight);

  const originRect = {
    left: rect.left,
    top: rect.top,
    width: rect.width || dockW,
    height: rect.height || dockH,
  };
  // The dock's own radius (a full pill in jelly) — read before "expanded" changes it.
  const dockRadius = getComputedStyle(card).borderRadius || "18px";
  // Rail avatars as docked right now (before "expanded" hides the rail): the
  // expand flies them into the list, the collapse flies them back here.
  card._tgRailAvatars = telegramAvatarRects(card, '.tg-rail', '.tg-avatar');
  card._tgRailScroll = card.querySelector('.tg-rail')?.scrollTop || 0;

  // Set destination dimensions immediately (1 Layout pass): the base card CSS
  // transitions geometry, which would animate the expand — kill it for this set.
  card.style.transition = "none";
  card.style.zIndex = "120";
  card.style.left = left + "px";
  card.style.top = top + "px";
  card.style.width = clampedW + "px";
  card.style.height = clampedH + "px";
  pos.expanded = true;
  // Mark expanded BEFORE the render: it both selects the expanded markup and
  // makes renderTelegramWidget's restore-guard bail (no recursive re-toggle).
  card.classList.add("expanded");

  // Build the real cached layout at its destination geometry, once. There is
  // no invented chat/message arrangement to jump away from at the handoff.
  card._tgPreparing = true;
  renderTelegramWidget(card, widgetId, { preparing: true });
  card.querySelector('.widget-body').inert = true;

  card._tgSurfaceTransitioning = true;
  surfaceManager.openSurface(card, {
    origin: originRect,
    axis: horizontalOnly ? "x" : "both",
    openOpacity: 1,
    closeOpacity: 1,
    originRadius: dockRadius,
    suppressContentAnimation: true,
    elevation: "120",
    closeOnOutsideClick: false,
    closeOnEscape: false,
    onTransitionStart: ({ open, duration }) => {
      syncSurfaceWash(card, open, duration);
      const list = telegramAvatarRects(card, '.tg-chat-list', '.tg-row-avatar');
      const rail = card._tgRailAvatars || new Map();
      flyTelegramAvatars(card, open ? rail : list, open ? list : rail, open, duration);
    },
    onOpened: () => {
      card._tgAvatarFlight?.settle();
      // Geometry already applied during the morph (transition was off) — hand
      // CSS transitions back for any future geometry changes.
      card.style.transition = "";
      activateTelegramExpandedRender(card, widgetId);
    },
    onClosed: () => {
      // Runs synchronously inside the close finalize, before the browser paints
      // — snap back to the dock in one task so there is no full-panel flash.
      card.style.transition = "none";
      card.classList.remove("expanded", "expand-left", "expand-right");
      pos.expanded = false;
      const cW = widgetsCanvas.clientWidth || window.innerWidth || 1920;
      const dW = pos.w || 56;
      const dH = pos.h || 520;
      card.style.left = Math.max(0, Math.min(pos.x ?? cW - dW - 12, cW - dW - 12)) + "px";
      card.style.top = (pos.y ?? 118) + "px";
      card.style.width = dW + "px";
      card.style.height = dH + "px";
      card.style.zIndex = "10";
      card._tgContentGeneration = (card._tgContentGeneration || 0) + 1;
      card._tgContentTimeline?.cleanup();
      card._tgContentTimeline = null;
      card._tgSurfaceTransitioning = false;
      card._tgPreparing = false;
      card._tgActivatePrepared = null;
      card.querySelector('.widget-body').inert = false;
      card._tgPendingUpdate = false;
      renderTelegramDock(card, widgetId);
      const rail = card.querySelector('.tg-rail');
      if (rail) rail.scrollTop = card._tgRailScroll || 0;
      // The ghosts land on avatars that only exist from this render on.
      card._tgAvatarFlight?.settle();
      // Restore transitions on the next frame, after this geometry set settles.
      requestAnimationFrame(() => { card.style.transition = ""; });
    },
  });

  if (pos.activeChatId) {
    openTelegramChatSession(pos.activeChatId);
  }
}

function collapseTelegramWidget(card, widgetId) {
  const scroller = card.querySelector('.tg-messages');
  if (scroller) card._tgDockedScroll = {
    generation: telegramRuntime.accountGeneration,
    chatId: scroller.dataset.chatId,
    threadId: scroller.dataset.threadId,
    state: captureTelegramScroll(scroller),
  };
  const pos = config.positions[widgetId] || (config.positions[widgetId] = defaultWidgetPosition("telegram"));
  if (pos.activeChatId) closeTelegramChatSession(pos.activeChatId);
  cancelActiveTelegramMorph(card);
  card._tgContentGeneration = (card._tgContentGeneration || 0) + 1;
  card._tgContentTimeline?.cleanup();
  card._tgContentTimeline = null;

  // The card is a surface now: let surfaceManager run the exit morph (reverse
  // of open) and restore geometry/content in its onClosed handler.
  const record = surfaceManager.getRecord(card);
  if (record) {
    card._tgSurfaceTransitioning = true;
    pos.expanded = false;
    if (record.state !== "closing") {
      surfaceManager.closeSurface(card);
    }
    // If it is already closing, its onClosed will do the dock restore.
    return;
  }

  // Fallback (no surface record — e.g. legacy/restored state): instant fold.
  const canvasW = widgetsCanvas.clientWidth || window.innerWidth || 1920;
  const dockW = pos.w || 56;
  const dockH = pos.h || 520;
  card.classList.remove("expanded", "expand-left", "expand-right");
  card._tgSurfaceTransitioning = false;
  card.style.transition = "";
  card.style.left = Math.max(0, Math.min(pos.x ?? canvasW - dockW - 12, canvasW - dockW - 12)) + "px";
  card.style.top = (pos.y ?? 118) + "px";
  card.style.width = dockW + "px";
  card.style.height = dockH + "px";
  pos.expanded = false;
  card.style.zIndex = "10";
  renderTelegramDock(card, widgetId);
}

export { renderTelegramWidget, toggleTelegramWidget, collapseTelegramWidget, openTelegramSendFilePicker, refreshTelegramChats, closeTelegramContextMenu, telegramRuntime, cachedTelegramCard, repairMojibake };
