// Appended to an isolated widget module by ui.ps1; runs in the actual WebView.
// All bridge calls in that module are stubbed; no user chat/read receipt is touched.
export async function runTelegramUiTests() {
  const checks = [];
  const check = (name, ok) => checks.push({ name, ok: !!ok });
  const frame = () => new Promise(resolve => requestAnimationFrame(resolve));
  const id = '__tg_ui_fixture';
  const card = document.createElement('div');
  card.className = 'widget-card telegram-widget expanded no-header';
  card.dataset.id = id;
  card.style.cssText = 'position:fixed;left:-4000px;top:0;width:560px;height:590px;visibility:hidden';
  card.innerHTML = '<div class="widget-body"></div>';
  document.body.appendChild(card);
  const chats = [{ id: 'fixture', title: 'Forum', initials: 'F', kind: 'group', isForum: false, preview: '', unread: 0 }];
  chats.push({ id: 'forum', title: 'Topics', initials: 'T', kind: 'group', isForum: true, preview: '', unread: 0 });
  const messages = Array.from({ length: 80 }, (_, i) => ({ id: String(i + 1), text: `Message ${i}`, from: 'Author', time: '' }));
  telegramRuntime.status = { authorized: true };
  telegramRuntime.statusLoaded = true;
  telegramRuntime.chatsLoaded = true;
  telegramRuntime.chats = chats;
  telegramRuntime.connectionState = 'ready';
  telegramRuntime.messages.set('fixture', messages);
  telegramRuntime.messagesLoaded.add('fixture');
  telegramRuntime.forumTopics.set('forum', [{ id: '7', title: 'General', lastMessage: '', unreadCount: 0 }, { id: '8', title: 'Other', lastMessage: '', unreadCount: 2 }]);
  config.positions[id] = { activeChatId: 'fixture', expanded: true };
  try {
    const nowTs = Math.floor(Date.now() / 1000);
    const pastTs = nowTs - 86400 * 3;
    check('time labels keep the locale format',
      formatTelegramTime(String(nowTs)) === new Date(nowTs * 1000).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' })
      && formatTelegramTime(String(pastTs)) === new Date(pastTs * 1000).toLocaleDateString([], { month: 'short', day: 'numeric' }));
    // One label per message on the open path: building a formatter per call
    // cost ~0.5 ms each, which made a 300-message chat freeze the click.
    const formatStart = performance.now();
    for (let i = 0; i < 600; i++) formatTelegramTime(String(nowTs - i * 3600));
    check('time labels reuse their formatters', performance.now() - formatStart < 30);
    renderTelegramWidget(card, id, { preparing: true });
    const body = card.querySelector('.widget-body');
    const scroller = body.querySelector('.tg-messages');
    const layout = body.querySelector('.tg-layout');
    const search = body.querySelector('.tg-chat-search');
    const composer = body.querySelector('.tg-input');
    // The click only lays out the visible end of the history; the rest is
    // written above it once the surface is open (first layout is ~0.3 ms a row).
    check('prepared real history tail, no mock', !!scroller && !body.querySelector('.tg-opening-shell') && scroller.querySelectorAll('.tg-message').length === TELEGRAM_PREPARED_TAIL_MESSAGES);
    check('prepared first paint at bottom', scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop < 2);
    check('prepared activation deferred', typeof card._tgActivatePrepared === 'function' && !scroller._replyWired);
    const tailLast = [...scroller.querySelectorAll('.tg-message')].at(-1);
    check('head fill completes history', fillTelegramHistoryHead(card, id) && scroller.querySelectorAll('.tg-message').length === 80);
    check('head fill keeps tail nodes and bottom', tailLast.isConnected && scroller.contains(tailLast) && scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop < 2);
    check('head fill leaves nothing to rebuild', card._tgSectionCache.thread === buildMessagesInnerHtml(chats[0], messages));
    wireTelegramMessagesSection(card, id, body, chats[0], { showAuth: true });
    check('live first paint at bottom', scroller.scrollHeight - scroller.clientHeight - scroller.scrollTop < 2);
    await frame();
    scroller.scrollTop = 420;
    const anchor = captureTelegramScroll(scroller);
    const anchorNode = scroller.querySelector(`[data-message-id="${anchor.messageId}"]`);
    const before = anchorNode.getBoundingClientRect().top - scroller.getBoundingClientRect().top;
    scroller.insertAdjacentHTML('afterbegin', '<div style="height:180px"></div>');
    wireTelegramMessagesSection(card, id, body, chats[0], { showAuth: true, previousScroll: anchor, prevChatId: 'fixture', contentOnly: true });
    check('prepend preserves anchor before paint', Math.abs(anchorNode.getBoundingClientRect().top - scroller.getBoundingClientRect().top - before) < 2);
    settleTelegramScroll(scroller);
    scroller.dispatchEvent(new WheelEvent('wheel'));
    scroller.scrollTop = 650;
    await frame();
    check('settle never undoes user scroll', Math.abs(scroller.scrollTop - 650) < 2);
    const mediaState = captureTelegramScroll(scroller);
    scroller._tgReadingPosition = mediaState;
    scroller.firstElementChild.style.height = '260px';
    scroller.firstElementChild.dispatchEvent(new Event('load'));
    const mediaAnchor = scroller.querySelector(`[data-message-id="${mediaState.messageId}"]`);
    check('media load preserves reading anchor', Math.abs(tgMessageOffsetInScroller(mediaAnchor, scroller) - scroller.scrollTop - mediaState.offset) < 2);
    await frame();
    const threadWidth = body.querySelector('.tg-thread').getBoundingClientRect().width;
    telegramRuntime.forumOpenChatId = 'forum';
    updateTelegramWidget(card, id);
    await frame();
    check('forum transition exists', layout.getAnimations().some(a => a.transitionProperty === 'grid-template-columns'));
    await new Promise(resolve => setTimeout(resolve, getDuration('motion') + 60));
    check('forum retains layout and composer', body.querySelector('.tg-layout') === layout && body.querySelector('.tg-input') === composer);
    check('forum retains thread width', Math.abs(body.querySelector('.tg-thread').getBoundingClientRect().width - threadWidth) < 2);
    check('chat search visible', parseFloat(getComputedStyle(search).fontSize) > 0 && search.getBoundingClientRect().width > 80);
    const topicSearch = body.querySelector('.tg-topic-search');
    topicSearch.value = 'other';
    topicSearch.dispatchEvent(new Event('input'));
    check('topic filter works', body.querySelectorAll('.tg-topic-row:not([hidden])').length === 1);
    check('topic filter preserves focus/DOM', body.querySelector('.tg-topic-search') === topicSearch);
    body.querySelector('[data-action="back-to-chats"]').click();
    check('back retains layout/search', body.querySelector('.tg-layout') === layout && body.querySelector('.tg-chat-search') === search);
    check('hidden topics inert', body.querySelector('.tg-topics-list').inert);
    await new Promise(resolve => setTimeout(resolve, getDuration('motion') + 60));
    telegramRuntime.forumOpenChatId = 'forum';
    updateTelegramWidget(card, id);
    card.style.width = '280px';
    await new Promise(resolve => setTimeout(resolve, getDuration('motion') + 60));
    check('narrow thread usable', body.querySelector('.tg-thread').getBoundingClientRect().width > layout.getBoundingClientRect().width * 0.5);
    check('narrow back visible', body.querySelector('.tg-topics-back-btn').getBoundingClientRect().width > 50);
    const panel = body.querySelector('.tg-panel');
    const strip = body.querySelector('.tg-connection-state');
    strip.style.display = 'block'; strip.textContent = 'Connecting';
    check('connection strip stays narrow', strip.getBoundingClientRect().height < 35);
    check('connection content contained', layout.getBoundingClientRect().bottom <= panel.getBoundingClientRect().bottom + 2);
    telegramRuntime.forumOpenChatId = null;
    scroller.scrollTop = 430;
    card._tgDockedScroll = { generation: telegramRuntime.accountGeneration, chatId: 'fixture', threadId: '', state: captureTelegramScroll(scroller) };
    const savedPosition = card._tgDockedScroll.state;
    body.replaceChildren();
    renderTelegramWidget(card, id, { preparing: true });
    const reopened = body.querySelector('.tg-messages');
    check('reopen preserves reading position before paint', Math.abs(tgMessageOffsetInScroller(reopened.querySelector(`[data-message-id="${savedPosition.messageId}"]`), reopened) - reopened.scrollTop - savedPosition.offset) < 2);
    body.replaceChildren();
    telegramRuntime.accountGeneration += 1;
    renderTelegramWidget(card, id, { preparing: true });
    const fresh = body.querySelector('.tg-messages');
    check('new account ignores old reading position', fresh.scrollHeight - fresh.clientHeight - fresh.scrollTop < 2);
    // History changed between the prepared click and the open: the tail on
    // screen is stale, so the head is not written over it; the update rebuilds.
    const editedLast = { ...messages[79], text: 'Edited while opening' };
    telegramRuntime.messages.set('fixture', [...messages.slice(0, 79), editedLast]);
    check('head fill refuses a changed history', !fillTelegramHistoryHead(card, id) && fresh.querySelectorAll('.tg-message').length === TELEGRAM_PREPARED_TAIL_MESSAGES);
    updateTelegramWidget(card, id);
    const rebuilt = body.querySelector('.tg-messages');
    check('changed history rebuilds in full', rebuilt.querySelectorAll('.tg-message').length === 80 && rebuilt.textContent.includes('Edited while opening'));
    // History changes while the chat is open: rows before the first changed
    // one keep their nodes, and the list equals a full build.
    const firstRow = rebuilt.querySelector('[data-message-id="1"]');
    const row79 = rebuilt.querySelector('[data-message-id="79"]');
    const arrived = { id: '81', text: 'Arrived while open', from: 'Author', time: '' };
    const grown = [...messages.slice(0, 79), editedLast, arrived];
    telegramRuntime.messages.set('fixture', grown);
    updateTelegramWidget(card, id);
    const sameHtml = (name, x, y) => { let i = 0; while (i < x.length && x[i] === y[i]) i++; const cut = (v) => v.slice(Math.max(0, i - 20), i + 70).split(String.fromCharCode(34)).join(String.fromCharCode(39)).split(String.fromCharCode(10)).join('~'); check(x === y ? name : `${name} @${i}/${x.length}/${y.length} A=${cut(x)} B=${cut(y)}`, x === y); };
    const rowsOf = (el) => Array.from(el.children).map(c => c.outerHTML).join('');
    const fullBuild = (list) => { const probe = document.createElement('div'); telegramRuntime.messages.set('fixture', list); probe.innerHTML = buildMessagesInnerHtml(chats[0], list); return rowsOf(probe); };
    check('new message keeps older rows', rebuilt.contains(firstRow) && rebuilt.contains(row79) && rebuilt.querySelectorAll('.tg-message').length === 81 && rebuilt.textContent.includes('Arrived while open'));
    sameHtml('appended list equals a full build', rowsOf(rebuilt), fullBuild(grown));
    const row80 = rebuilt.querySelector('[data-message-id="80"]');
    const lastEdit = { ...arrived, text: 'Edited again' };
    telegramRuntime.messages.set('fixture', [...grown.slice(0, 80), lastEdit]);
    updateTelegramWidget(card, id);
    check('edit of the last row keeps the rest', rebuilt.contains(firstRow) && rebuilt.contains(row80) && rebuilt.textContent.includes('Edited again') && !rebuilt.textContent.includes('Arrived while open'));
    sameHtml('edited list equals a full build', rowsOf(rebuilt), fullBuild([...grown.slice(0, 80), lastEdit]));
    telegramRuntime.messages.set('fixture', messages.slice(0, 60));
    updateTelegramWidget(card, id);
    check('shorter history keeps 60 rows', rebuilt.querySelectorAll('.tg-message').length === 60);
    sameHtml('shorter list equals a full build', rowsOf(rebuilt), fullBuild(messages.slice(0, 60)));
    // A refresh during the open, with the prepared tail on screen and its head
    // fill pending: the tail is patched and the fill stays valid.
    telegramRuntime.messages.set('fixture', messages);
    body.replaceChildren();
    renderTelegramWidget(card, id, { preparing: true });
    const pending = body.querySelector('.tg-messages');
    pending.dataset.chatId = 'fixture'; pending.dataset.threadId = '';
    const tailRow = pending.querySelector('[data-message-id="70"]');
    const grownMessages = [...messages, { id: '81', text: 'Arrived during open', from: 'Author', time: '' }];
    telegramRuntime.messages.set('fixture', grownMessages);
    updateTelegramWidget(card, id);
    check('refresh during open patches the tail', pending.contains(tailRow) && pending.textContent.includes('Arrived during open') && pending._tgHeadFill);
    check('head fill follows the patched tail', fillTelegramHistoryHead(card, id));
    sameHtml('head fill after a patch equals a full build', rowsOf(pending), fullBuild(grownMessages));
    telegramRuntime.messages.set('fixture', messages);
    card.style.width = '560px';
    chats.push({ id: 'next', title: 'Next', initials: 'N', kind: 'private', unread: 0 }, { id: 'third', title: 'Third', initials: 'T', kind: 'private', unread: 0 });
    for (const chatId of ['next', 'third']) {
      telegramRuntime.messages.set(chatId, messages);
      telegramRuntime.messagesLoaded.add(chatId);
      telegramRuntime.loadingMessages.add(chatId); // isolate the animated handoff from refresh
    }
    renderTelegramWidget(card, id, { preparing: true });
    const outgoing = body.querySelector('.tg-layout > .tg-thread');
    wireTelegramComposer(card, id, outgoing, chats[0]);
    const oldInput = outgoing.querySelector('.tg-input');
    const origin = body.querySelector('[data-chat-id="next"]');
    telegramSetActiveChat(card, id, 'next', origin);
    const firstTimeline = card._activeMorphAnim;
    const plane = card._activeMorphCard;
    const incoming = plane.querySelector('.tg-thread');
    const incomingMessages = incoming.querySelector('.tg-messages');
    const panelRect = body.querySelector('.tg-panel').getBoundingClientRect();
    const rowRect = origin.getBoundingClientRect();
    const clip = firstTimeline.tracks[0].keyframes[0].clipPath;
    check('morph starts at row in panel coordinates', clip.includes(`${rowRect.left - panelRect.left}px`) && plane.getBoundingClientRect().width === panelRect.width);
    check('morph blocks outgoing input and preview', outgoing.inert && plane.inert && plane.getAttribute('aria-hidden') === 'true');
    oldInput.value = 'must not send';
    oldInput.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await Promise.resolve();
    check('outgoing Enter cannot send during transition', !fixtureCalls.some(call => call.command === 'telegram_send_message'));
    check('switch prepares only the history tail', incomingMessages.querySelectorAll('.tg-message').length === TELEGRAM_PREPARED_TAIL_MESSAGES);
    const previewWidth = incoming.getBoundingClientRect().width;
    // The head is written above the tail, so the reading position is the
    // distance from the bottom, not the raw scrollTop.
    const previewFromBottom = incomingMessages.scrollHeight - incomingMessages.scrollTop;
    const previewMessage = incomingMessages.querySelector(".tg-message");
    card._tgPendingUpdate = true;
    firstTimeline.finish();
    await firstTimeline.finished;
    check('unchanged pending refresh retains message nodes', incomingMessages.contains(previewMessage) && incomingMessages.querySelectorAll('.tg-message').length === 80);
    check('handoff retains prepared thread and scroller', body.querySelector('.tg-layout > .tg-thread') === incoming && incoming.querySelector('.tg-messages') === incomingMessages);
    check('handoff retains width and history position', Math.abs(incoming.getBoundingClientRect().width - previewWidth) < 2 && Math.abs(incomingMessages.scrollHeight - incomingMessages.scrollTop - previewFromBottom) < 2);
    const input = incoming.querySelector('.tg-input');
    input.value = 'new draft';
    input.dispatchEvent(new Event('input'));
    check('incoming composer is wired to selected chat', telegramRuntime.drafts.get('next') === 'new draft');
    input.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    await Promise.resolve(); await Promise.resolve();
    check('incoming Enter routes to selected chat', fixtureCalls.some(call => call.command === 'telegram_send_message' && call.args.request.chatId === 'next'));
    telegramSetActiveChat(card, id, 'fixture', body.querySelector('[data-chat-id="fixture"]'));
    const cancelled = card._activeMorphAnim;
    telegramSetActiveChat(card, id, 'third', body.querySelector('[data-chat-id="third"]'));
    const winner = card._activeMorphAnim;
    check('rapid clicks retain only the latest plane', cancelled.state === 'cancelled' && body.querySelectorAll('.tg-surface-morph').length === 1);
    winner.finish(); await winner.finished;
    check('latest selection alone commits', body.querySelector('.tg-messages').dataset.chatId === 'third' && !card._tgTransitioning && !body.querySelector('.tg-surface-morph'));
    chats.push({ id: 'empty', title: 'Empty', initials: 'E', kind: 'private', unread: 0 });
    // Put an origin row in the retained list without invoking the backend.
    body.querySelector('.tg-chat-list').innerHTML = buildChatListHtml(chats, 'third', telegramChatListState(chats));
    telegramRuntime.historyExhausted.add('empty');
    telegramSetActiveChat(card, id, 'empty', body.querySelector('[data-chat-id="empty"]'));
    const emptyTimeline = card._activeMorphAnim;
    check('cold history renders loading before first frame', card._activeMorphCard.querySelector('.tg-state.loading')?.textContent === 'Загрузка сообщений');
    await Promise.resolve(); await Promise.resolve();
    emptyTimeline.finish(); await emptyTimeline.finished;
    check('empty reply replaces loading after handoff', body.querySelector('.tg-state')?.textContent === 'Сообщений нет' && !body.querySelector('.tg-state.loading'));
    fixtureReducedMotion = true;
    telegramSetActiveChat(card, id, 'next', body.querySelector('[data-chat-id="next"]'));
    check('reduced motion commits immediately with wired input', !card._tgTransitioning && !card._activeMorphCard && body.querySelector('.tg-messages').dataset.chatId === 'next' && body.querySelector('.tg-thread')._tgComposerWired);
    check('reduced motion starts at history bottom', body.querySelector('.tg-messages').scrollHeight - body.querySelector('.tg-messages').clientHeight - body.querySelector('.tg-messages').scrollTop < 2);
    fixtureReducedMotion = false;
    let resolveEdit;
    fixtureReplies.set('telegram_edit_message', new Promise(resolve => { resolveEdit = resolve; }));
    telegramRuntime.editMessage = new Map([['next', { id: '1' }]]);
    const editInput = body.querySelector('.tg-input');
    editInput.value = 'stale edit';
    editInput.dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    telegramRuntime.accountGeneration += 1;
    telegramRuntime.sending = true; // a send owned by the new generation
    resolveEdit({ id: '1', text: 'stale edit' });
    await Promise.resolve(); await Promise.resolve(); await Promise.resolve();
    check('late edit cannot change new account or its send guard', telegramRuntime.sending && !telegramRuntime.messages.get('next').some(message => message.text === 'stale edit'));
    telegramRuntime.sending = false;
    telegramRuntime.editMessage.clear();
    renderTelegramWidget(card, id, { preparing: true });
    wireTelegramComposer(card, id, body.querySelector('.tg-thread'), chats.find(chat => chat.id === 'next'));
    let resolveAttachment;
    fixtureReplies.set('telegram_send_attachment', new Promise(resolve => { resolveAttachment = resolve; }));
    telegramRuntime.pendingExternalFile = 'old-file';
    body.querySelector('.tg-input').value = 'caption';
    body.querySelector('.tg-input').dispatchEvent(new KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    telegramRuntime.accountGeneration += 1;
    telegramRuntime.pendingExternalFile = 'new-file';
    telegramRuntime.sending = true;
    resolveAttachment({ id: '999' });
    await Promise.resolve(); await Promise.resolve(); await Promise.resolve();
    check('late attachment cannot clear new account file or send guard', telegramRuntime.pendingExternalFile === 'new-file' && telegramRuntime.sending);
    telegramRuntime.sending = false;
    telegramSetActiveChat(card, id, 'third', body.querySelector('[data-chat-id="third"]'));
    const cancelledByAccount = card._activeMorphAnim;
    clearTelegramAccountState();
    check('account reset cancels morph immediately', cancelledByAccount.state === 'cancelled' && !body.querySelector('.tg-surface-morph') && !card._tgTransitioning);
    return { passed: checks.filter(c => c.ok).length, total: checks.length, checks };
  } finally {
    card.remove();
    delete config.positions[id];
  }
}
