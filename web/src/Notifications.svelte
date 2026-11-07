<script lang="ts">
  // Limits account effects to identity changes.
  import { untrack } from 'svelte';
  // Keeps dialog dismissal and focus accessible.
  import { Dialog } from 'bits-ui';
  // Identifies notifications and dialog close controls.
  import Bell from '@lucide/svelte/icons/bell';
  import X from '@lucide/svelte/icons/x';
  // Opens explicit recovery for exhausted creation journals.
  import CreationRecovery from './CreationRecovery.svelte';

  type Notice = { id: string; kind: string; message: string; threadId: string | null;
    createdAt: number; read: boolean };
  let { account, onconversation }: { account?: string;
    onconversation: (id: string) => void | Promise<void> } = $props();
  let open = $state(false);
  let notices = $state<Notice[]>([]);
  let error = $state('');
  let loading = $state(false);
  let saving = $state('');
  let storageError = $state(false);
  let generation = 0;
  let request = 0;
  const unread = $derived(notices.filter(notice => !notice.read).length);

  $effect(() => {
    const identity = account;
    generation += 1;
    notices = []; error = ''; saving = ''; storageError = false;
    if (!identity) { open = false; return; }
    untrack(() => { void refresh(identity); });
    const timer = setInterval(() => {
      if (!document.hidden) void refresh(identity);
    }, 10000);
    return () => { clearInterval(timer); generation += 1; };
  });

  // Rejects malformed notice fields before they reach the reactive list or date formatter.
  function validNotice(value: unknown): value is Notice {
    if (!value || typeof value !== 'object') return false;
    const notice = value as Record<string, unknown>;
    return typeof notice.id === 'string' && typeof notice.kind === 'string' &&
      typeof notice.message === 'string' && typeof notice.read === 'boolean' &&
      (notice.threadId === null || typeof notice.threadId === 'string') &&
      typeof notice.createdAt === 'number' && Number.isFinite(notice.createdAt) &&
      Number.isFinite(new Date(notice.createdAt * 1000).getTime());
  }

  // Refreshes retained notices and discards responses from earlier accounts or requests.
  async function refresh(identity = account) {
    if (!identity || saving) return;
    const current = generation;
    const version = ++request;
    loading = true;
    try {
      const response = await fetch('/api/notifications', { signal: AbortSignal.timeout(8000) });
      if (!response.ok) {
        if (current === generation && version === request && [401, 403].includes(response.status)) notices = [];
        throw new Error();
      }
      const result = await response.json();
      if (current !== generation || version !== request) return;
      if (!Array.isArray(result?.notifications) || result.notifications.length > 1000 ||
        !result.notifications.every(validNotice)) throw new Error();
      notices = result.notifications;
      storageError = result.storageError === true;
      error = '';
    } catch {
      if (current === generation && version === request) error = 'Could not load notifications. Try again.';
    } finally {
      if (current === generation && version === request) loading = false;
    }
  }

  // Opens the global list and requests its latest persisted state.
  function opened(value: boolean) {
    open = value;
    if (value) void refresh();
  }

  // Marks every notice read in one request.
  async function markAllRead() {
    const current = generation;
    request += 1;
    saving = 'all';
    error = '';
    try {
      const response = await fetch('/api/notifications/read-all', {
        method: 'POST', signal: AbortSignal.timeout(8000), headers: { 'Content-Type': 'application/json', 'X-MiniClaw-Request': '1' },
        body: '{}',
      });
      if (!response.ok) throw new Error();
      if (current === generation) notices = notices.map(item => ({ ...item, read: true }));
    } catch {
      if (current === generation) error = 'Could not mark the notices as read. Try again.';
    } finally {
      if (current === generation) { saving = ''; loading = false; }
    }
  }

  // Removes read notices after the backend confirms this account's deletion.
  async function clearRead() {
    const current = generation;
    request += 1;
    saving = 'clear';
    error = '';
    try {
      const response = await fetch('/api/notifications/clear-read', {
        method: 'POST', signal: AbortSignal.timeout(8000),
        headers: { 'Content-Type': 'application/json', 'X-MiniClaw-Request': '1' },
        body: '{}',
      });
      if (!response.ok) throw new Error();
      if (current === generation) notices = notices.filter(item => !item.read);
    } catch {
      if (current === generation) error = 'Could not clear read notifications. Try again.';
    } finally {
      if (current === generation) { saving = ''; loading = false; }
    }
  }

  // Changes read state only after the backend acknowledges the current account's write.
  async function markRead(notice: Notice) {
    const current = generation;
    request += 1;
    saving = notice.id;
    error = '';
    try {
      const response = await fetch(`/api/notifications/${encodeURIComponent(notice.id)}/read`, {
        method: 'POST', signal: AbortSignal.timeout(8000), headers: { 'Content-Type': 'application/json', 'X-MiniClaw-Request': '1' },
        body: '{}',
      });
      if (!response.ok) throw new Error();
      if (current === generation) notices = notices.map(item =>
        item.id === notice.id ? { ...item, read: true } : item);
    } catch {
      if (current === generation) error = 'Could not mark this notice as read. Try again.';
    } finally {
      if (current === generation) { saving = ''; loading = false; }
    }
  }

  // Opens the affected conversation through its normal ownership-checked loading path.
  function visit(id: string) {
    open = false;
    void onconversation(id);
  }
</script>

<Dialog.Root {open} onOpenChange={opened}>
  <Dialog.Trigger class="icon-button relative" aria-label="Notifications" disabled={!account}>
    <Bell size={18} aria-hidden="true" />
    {#if unread}<span class="absolute right-0.5 top-0.5 flex h-4 min-w-4 items-center justify-center rounded-full bg-brand px-1 text-[10px] font-semibold leading-none text-brand-foreground ring-2 ring-background" aria-hidden="true">{unread > 9 ? '9+' : unread}</span>{/if}
  </Dialog.Trigger>
  <Dialog.Portal>
    <Dialog.Overlay class="fixed inset-0 z-40 bg-black/30" />
    <Dialog.Content class="fixed left-1/2 top-1/2 z-50 flex max-h-[85dvh] w-[min(640px,calc(100vw-2rem))] -translate-x-1/2 -translate-y-1/2 flex-col rounded-xl border border-border bg-muted p-5 shadow-xl">
      <div class="flex items-center justify-between gap-3">
        <Dialog.Title class="text-lg font-semibold">Notifications</Dialog.Title>
        <Dialog.Close class="icon-button" aria-label="Close notifications"><X size={18} aria-hidden="true" /></Dialog.Close>
      </div>
      <Dialog.Description class="my-3 text-sm text-muted-foreground">{unread} unread. Recent response and connection failures are saved here.</Dialog.Description>
      <div class="flex flex-wrap items-center gap-4 text-sm">
        <button class="underline" disabled={loading || !!saving} onclick={() => refresh()}>Refresh notifications</button>
        <button class="underline disabled:no-underline disabled:opacity-50" disabled={loading || !!saving || !unread} onclick={markAllRead}>Mark all as read</button>
        <button class="underline disabled:no-underline disabled:opacity-50" disabled={loading || !!saving || notices.length === unread} onclick={clearRead}>Clear read</button>
      </div>
      {#if error}<p class="my-3 text-sm text-red-700" role="alert">{error}</p>{/if}
      {#if storageError}<p class="my-3 text-sm text-red-700" role="alert">Some failures could not be saved. Check application storage.</p>{/if}
      <div class="mt-3 min-h-0 overflow-y-auto">
        {#if !notices.length}<p class="py-6 text-sm text-muted-foreground">{loading ? 'Loading notifications…' : 'No saved notifications.'}</p>{/if}
        <ul class="divide-y divide-line">
          {#each notices as notice (notice.id)}
            <li class="py-4">
              <div class="flex items-start justify-between gap-3">
                <p class="text-sm" class:font-semibold={!notice.read}>{notice.message}</p>
                <span class="shrink-0 text-xs text-muted-foreground">{notice.read ? 'Read' : 'Unread'}</span>
              </div>
              <time class="mt-1 block text-xs text-muted-foreground" datetime={new Date(notice.createdAt * 1000).toISOString()}>{new Date(notice.createdAt * 1000).toLocaleString()}</time>
              <div class="mt-3 flex flex-wrap gap-4 text-sm">
                {#if notice.threadId}<button class="underline" onclick={() => visit(notice.threadId!)}>Open conversation</button>{/if}
                {#if !notice.read}<button class="underline" disabled={!!saving} onclick={() => markRead(notice)}>Mark as read</button>{/if}
              </div>
            </li>
          {/each}
        </ul>
      </div>
      <CreationRecovery {account} />
    </Dialog.Content>
  </Dialog.Portal>
</Dialog.Root>
