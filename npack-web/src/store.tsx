import { useEffect, useMemo, useState } from 'react';
import { NPool, NRelay1 } from '@nostrify/nostrify';
import type { NostrFilter } from '@nostrify/nostrify';
import { Activity, AlertTriangle, ArrowDownToLine, ArrowLeft, Check, ChevronDown, FileUp, Heart, LayoutGrid, LoaderCircle, Package, RefreshCw, Search, Settings2, ShieldCheck, Trash2, Upload, X } from 'lucide-react';

type Page = 'browse' | 'installed' | 'updates' | 'health' | 'settings';
type Json = Record<string, unknown>;
type PackageRow = Json & { name: string; version: string; publisher: string; npub?: string; trust?: string; app?: Json };
type OperationProgress = { label: string; stage: string; package?: string; detail?: string };

const launchToken = new URLSearchParams(window.location.hash.slice(1)).get('token');
const token = launchToken ?? sessionStorage.getItem('npack-web-token') ?? '';
if (launchToken) {
  sessionStorage.setItem('npack-web-token', launchToken);
  window.history.replaceState(null, '', window.location.pathname + window.location.search);
}
let requestId = 1;

async function rpc<T>(method: string, params: Json = {}): Promise<T> {
  const response = await fetch('/api/rpc', {
    method: 'POST',
    headers: { 'content-type': 'application/json', authorization: `Bearer ${token}` },
    body: JSON.stringify({ id: requestId++, method, params }),
  });
  const payload = await response.json() as { result?: T; error?: string; error_code?: string };
  if (!response.ok || payload.error) throw new Error(payload.error ?? `Request failed (${response.status})`);
  return payload.result as T;
}

function asList<T>(value: unknown): T[] { return Array.isArray(value) ? value as T[] : []; }
function str(value: unknown, fallback = ''): string { return typeof value === 'string' ? value : fallback; }
function displayName(row: PackageRow): string { return str((row.app as Json | undefined)?.name, row.name); }
function summary(row: PackageRow): string { return str((row.app as Json | undefined)?.summary, 'A Nostr-published package for your Linux system.'); }
function packageCapabilities(row: PackageRow): string[] {
  const details = row.details as Json | undefined;
  return asList<string>(details?.post_install).some((effect) => effect.startsWith('register-service')) ? ['service-manager'] : [];
}
function publisherShort(value: string): string { return value.startsWith('npub1') ? `${value.slice(0, 12)}…${value.slice(-6)}` : `${value.slice(0, 10)}…`; }
function size(value: unknown): string {
  if (typeof value !== 'number' || value <= 0) return 'Size not listed';
  const units = ['B', 'KB', 'MB', 'GB']; let n = value; let i = 0;
  while (n >= 1024 && i < units.length - 1) { n /= 1024; i += 1; }
  return `${n < 10 ? n.toFixed(1) : Math.round(n)} ${units[i]}`;
}

export default function Store() {
  const [page, setPage] = useState<Page>('browse');
  const [config, setConfig] = useState<Json>({});
  const [installed, setInstalled] = useState<PackageRow[]>([]);
  const [results, setResults] = useState<PackageRow[]>([]);
  const [categories, setCategories] = useState<{ category: string; count: number }[]>([]);
  const [query, setQuery] = useState('');
  const [category, setCategory] = useState('');
  const [selected, setSelected] = useState<PackageRow | null>(null);
  const [updates, setUpdates] = useState<Json[]>([]);
  const [health, setHealth] = useState<Json | null>(null);
  const [busy, setBusy] = useState(false);
  const [operation, setOperation] = useState<OperationProgress | null>(null);
  const [loading, setLoading] = useState(false);
  const [notice, setNotice] = useState('');
  const [error, setError] = useState('');
  const [trustConfirm, setTrustConfirm] = useState(false);
  const [serviceAllowed, setServiceAllowed] = useState(false);
  const [removeConfirm, setRemoveConfirm] = useState<PackageRow | null>(null);
  const [upload, setUpload] = useState<{ id: string; preview: Json } | null>(null);
  const [localAllowed, setLocalAllowed] = useState<string[]>([]);
  const [dropActive, setDropActive] = useState(false);
  const [nostrCount, setNostrCount] = useState<number | null>(null);

  const relays = useMemo(() => asList<string>(config.relays), [config]);

  async function loadInstalled() {
    setInstalled(await rpc<PackageRow[]>('ListInstalled'));
  }

  async function browse(refresh = false, categoryFilter = category, queryFilter = query, relayList = relays) {
    setLoading(true); setError('');
    try {
      const relayEvents = new NPool({
        open: (url) => new NRelay1(url),
        reqRouter: (filters: NostrFilter[]) => new Map(relayList.map((url) => [url, filters])),
        eventRouter: () => [],
        eoseTimeout: 1500,
      });
      const discover = (async () => {
        try { return await relayEvents.query([{ kinds: [9900], limit: 1000 }]); }
        catch { return []; }
        finally { await relayEvents.close(); }
      })();
      const [found, rows, cats] = await Promise.all([
        discover,
        rpc<PackageRow[]>('Search', { query: queryFilter, relay: relayList, include_untrusted: true, host_only: true, category: categoryFilter || undefined, refresh, sort: 'recent', limit: 96 }),
        rpc<{ category: string; count: number }[]>('ListCategories', { relay: relayList, include_untrusted: true, host_only: true }),
      ]);
      const discoveredIds = new Set(found.map((event) => event.id));
      setNostrCount(found.length);
      setResults(rows.filter((row) => discoveredIds.has(str(row.release_event_id))));
      setCategories(cats);
    } catch (reason) { setError(reason instanceof Error ? reason.message : 'Could not load the software catalogue.'); }
    finally { setLoading(false); }
  }

  async function loadBase() {
    setError('');
    try {
      const nextConfig = await rpc<Json>('GetConfig'); setConfig(nextConfig);
      await loadInstalled();
      void browse(false, category, query, asList<string>(nextConfig.relays));
    } catch (reason) { setError(reason instanceof Error ? reason.message : 'Could not connect to npackd.'); }
  }

  useEffect(() => { void loadBase(); }, []);
  useEffect(() => {
    if (page === 'updates') void rpc<Json[]>('CheckUpdates').then(setUpdates).catch((reason: unknown) => setError(String(reason)));
    if (page === 'health') void rpc<Json>('RunDoctor', { user: true }).then(setHealth).catch((reason: unknown) => setError(String(reason)));
  }, [page]);

  async function install(row: PackageRow, trustOnce: boolean) {
    setBusy(true); setError(''); setNotice(''); setOperation({ label: `Installing ${row.name}`, stage: 'starting' }); setTrustConfirm(false);
    try {
      const value = await rpc<Json>('Install', { package: `${row.publisher}/${row.name}`, relay: relays, trust_once: trustOnce && row.trust !== 'trusted' ? [row.publisher] : [], allow_capability: serviceAllowed ? ['service-manager'] : [], async: true });
      await pollTransaction(value, `Installing ${row.name}`); await refreshAfterMutation(); setNotice(`${row.name} is installed.`);
      setSelected(null);
    } catch (reason) { setError(reason instanceof Error ? reason.message : 'Install failed.'); }
    finally { setBusy(false); setOperation(null); }
  }

  async function pollTransaction(value: Json, label: string) {
    const id = value.transaction_id;
    if (typeof id !== 'number') return;
    for (let attempt = 0; attempt < 600; attempt += 1) {
      const state = await rpc<Json>('GetTransaction', { transaction_id: id });
      const progress = state.progress as Json | undefined;
      if (state.status === 'running') setOperation({ label, stage: str(progress?.stage, 'working'), package: str(progress?.package) || undefined, detail: str(progress?.detail) || undefined });
      if (state.status === 'succeeded') { setOperation({ label, stage: 'completed' }); return; }
      if (state.status === 'failed' || state.status === 'cancelled') throw new Error(str(state.error, 'Package operation failed.'));
      await new Promise((resolve) => window.setTimeout(resolve, 500));
    }
    throw new Error('The operation is taking longer than expected. Check the installed list for its result.');
  }

  async function refreshAfterMutation() {
    await loadInstalled();
    if (page === 'updates') setUpdates(await rpc<Json[]>('CheckUpdates'));
  }

  async function remove(row: PackageRow) {
    setBusy(true); setRemoveConfirm(null);
    try { await rpc('Remove', { package: `${row.publisher}/${row.name}` }); await loadInstalled(); setNotice(`${row.name} was removed.`); }
    catch (reason) { setError(reason instanceof Error ? reason.message : 'Removal failed.'); }
    finally { setBusy(false); }
  }

  async function updateAll() {
    setBusy(true); setError(''); setNotice(''); setOperation({ label: 'Updating installed software', stage: 'starting' });
    try { await pollTransaction(await rpc<Json>('Update', { relay: relays, async: true }), 'Updating installed software'); await refreshAfterMutation(); setNotice('Software is up to date.'); }
    catch (reason) { setError(reason instanceof Error ? reason.message : 'Update failed.'); }
    finally { setBusy(false); setOperation(null); }
  }

  async function refreshCatalogue() {
    setBusy(true);
    try { await rpc('RefreshCatalogue', { relay: relays }); await browse(); setNotice('Catalogue refreshed.'); }
    catch (reason) { setError(reason instanceof Error ? reason.message : 'Catalogue refresh failed.'); }
    finally { setBusy(false); }
  }

  async function inspectFile(file: File) {
    if (!file.name.toLowerCase().endsWith('.npk')) { setError('Choose a .npk package file.'); return; }
    setBusy(true); setError('');
    try {
      const response = await fetch('/api/local/inspect', { method: 'POST', headers: { authorization: `Bearer ${token}`, 'content-type': 'application/octet-stream' }, body: file });
      const body = await response.json() as { preview?: Json; error?: string };
      const uploadId = str(body.preview?.upload_id);
      if (!response.ok || !uploadId || !body.preview) throw new Error(body.error ?? 'Could not inspect this package.');
      setUpload({ id: uploadId, preview: body.preview }); setLocalAllowed([]);
    } catch (reason) { setError(reason instanceof Error ? reason.message : 'Could not inspect this package.'); }
    finally { setBusy(false); }
  }

  async function installUpload() {
    if (!upload) return;
    setBusy(true);
    try {
      const response = await fetch('/api/local/install', { method: 'POST', headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json' }, body: JSON.stringify({ upload_id: upload.id, allow_capability: localAllowed }) });
      const payload = await response.json() as { result?: Json; error?: string };
      if (!response.ok || payload.error || !payload.result) throw new Error(payload.error ?? 'Local install failed.');
      const transaction = payload.result;
      setOperation({ label: `Installing ${str(upload.preview.name)}`, stage: 'starting' });
      await pollTransaction(transaction, `Installing ${str(upload.preview.name)}`); setUpload(null); await refreshAfterMutation(); setNotice('Local package installed.');
    } catch (reason) { setError(reason instanceof Error ? reason.message : 'Local install failed.'); }
    finally { setBusy(false); setOperation(null); }
  }

  function discardUpload() {
    if (busy) return;
    if (upload) void fetch('/api/local/discard', { method: 'POST', headers: { authorization: `Bearer ${token}`, 'content-type': 'application/json' }, body: JSON.stringify({ upload_id: upload.id }) });
    setUpload(null);
  }

  function select(row: PackageRow) { setSelected(row); setServiceAllowed(false); setTrustConfirm(false); }

  return <div className={`shell ${dropActive ? 'drop-active' : ''}`}
    onDragOver={(event) => { event.preventDefault(); setDropActive(true); }}
    onDragLeave={(event) => { if (event.currentTarget === event.target) setDropActive(false); }}
    onDrop={(event) => { event.preventDefault(); setDropActive(false); const file = event.dataTransfer.files.item(0); if (file) void inspectFile(file); }}>
    <aside className="rail">
      <button className="brand" onClick={() => { setPage('browse'); setSelected(null); }}><span className="brand-mark"><Package size={21} /></span><span><b>npack</b><small>SOFTWARE, SIMPLY</small></span></button>
      <div className="rail-label">LIBRARY</div>
      <nav>
        <NavButton active={page === 'browse'} icon={<LayoutGrid />} label="Discover" onClick={() => { setPage('browse'); setSelected(null); }} />
        <NavButton active={page === 'installed'} icon={<Package />} label="Installed" count={installed.length} onClick={() => { setPage('installed'); setSelected(null); }} />
        <NavButton active={page === 'updates'} icon={<ArrowDownToLine />} label="Updates" onClick={() => { setPage('updates'); setSelected(null); }} />
      </nav>
      <div className="rail-bottom">
        <NavButton active={page === 'health'} icon={<Activity />} label="System health" onClick={() => { setPage('health'); setSelected(null); }} />
        <NavButton active={page === 'settings'} icon={<Settings2 />} label="Preferences" onClick={() => { setPage('settings'); setSelected(null); }} />
        <div className="scope"><span className="scope-dot" />Installing for <b>you</b><ChevronDown size={14} /></div>
        <div className="rail-foot"><ShieldCheck size={16} /> Signed releases. Verified files.</div>
      </div>
    </aside>
    <main className="main">
      <header className="topbar"><div className="crumb">npack <span>/</span> {selected ? displayName(selected) : pageTitle(page)}</div><div className="top-actions"><button className="icon-button" title="Refresh" onClick={() => page === 'browse' ? void browse(true) : void loadInstalled()}><RefreshCw size={17} /></button><span className="connection"><i /> LOCAL SERVICE</span></div></header>
      <div className="content">
        {error && <div className="banner error-banner"><AlertTriangle size={17} /><span>{error}</span><button onClick={() => setError('')}><X size={16} /></button></div>}
        {notice && <div className="banner success-banner"><Check size={17} /><span>{notice}</span><button onClick={() => setNotice('')}><X size={16} /></button></div>}
        {operation && <div className="operation-panel" role="status" aria-live="polite"><LoaderCircle className="spin" size={18} /><div className="operation-copy"><b>{operation.label}</b><span>{progressText(operation)}</span>{operation.package && <small>{operation.package}</small>}</div><div className="operation-track"><span /></div></div>}
        {selected ? <PackageDetails row={selected} onBack={() => setSelected(null)} onInstall={() => selected.trust === 'trusted' && packageCapabilities(selected).length === 0 ? void install(selected, false) : setTrustConfirm(true)} installed={installed.some((item) => item.name === selected.name && item.publisher === selected.publisher)} />
          : page === 'browse' ? <section>
            <div className="eyebrow">THE NPACK CATALOGUE</div><div className="hero-row"><div><h1>Find your next<br /><em>favourite tool.</em></h1><p>Software published on Nostr, checked before it reaches your system.</p></div><button className="file-button" onClick={() => document.getElementById('npk-file')?.click()}><Upload size={17} /> Install a .npk file</button><input id="npk-file" hidden type="file" accept=".npk" onChange={(event) => { const file = event.target.files?.[0]; if (file) void inspectFile(file); event.currentTarget.value = ''; }} /></div>
            <label className="searchbox"><Search size={19} /><input value={query} onChange={(event) => setQuery(event.target.value)} onKeyDown={(event) => { if (event.key === 'Enter') void browse(); }} placeholder="Search apps, tools and utilities" /><kbd>Enter</kbd></label>
            <div className="catalogue-meta"><span>{loading ? 'Finding packages…' : `${results.length} packages`}</span><span>{nostrCount === null ? 'Nostrify discovery' : `Nostrify checked ${nostrCount} release events`}</span><button onClick={() => void refreshCatalogue()} disabled={busy}><RefreshCw size={14} /> Refresh catalogue</button></div>
            <div className="browse-layout"><aside className="categories"><div className="section-label">CATEGORIES</div><button className={!category ? 'chosen' : ''} onClick={() => { setCategory(''); void browse(false, ''); }}>Everything <span>{results.length}</span></button>{categories.map((item) => <button key={item.category} className={category === item.category ? 'chosen' : ''} onClick={() => { setCategory(item.category); void browse(false, item.category); }}>{item.category}<span>{item.count}</span></button>)}</aside>
              <div className="results-grid">{loading ? <div className="empty"><LoaderCircle className="spin" /><span>Connecting to relays…</span></div> : results.length ? results.map((row) => <PackageCard key={`${row.publisher}/${row.name}/${row.version}`} row={row} onClick={() => select(row)} />) : <div className="empty"><Search size={24} /><b>No packages found</b><span>Try a different search or refresh the catalogue.</span></div>}</div>
            </div>
          </section> : page === 'installed' ? <section><PageHeading eyebrow="YOUR LIBRARY" title="Installed software" description="The packages currently on this computer." action={<button className="quiet-button" onClick={() => void loadInstalled()}><RefreshCw size={15} /> Refresh</button>} />{installed.length ? <div className="installed-list">{installed.map((row) => <div className="installed-row" key={`${row.publisher}/${row.name}`}><div className="app-glyph"><Package /></div><div className="installed-info"><b>{displayName(row)}</b><span>{row.version} <i>·</i> {publisherShort(row.npub ?? row.publisher)}</span></div><button className="quiet-button danger-text" onClick={() => setRemoveConfirm(row)}><Trash2 size={15} /> Remove</button></div>)}</div> : <EmptyState title="Nothing installed yet" detail="Browse the catalogue to find software for your system." action={<button className="primary-button" onClick={() => setPage('browse')}>Explore packages</button>} />}</section>
            : page === 'updates' ? <section><PageHeading eyebrow="KEEP THINGS CURRENT" title="Updates" description="Review and install newer package versions." action={<button className="primary-button" onClick={() => void updateAll()} disabled={busy}><ArrowDownToLine size={16} /> Update all</button>} />{updates.length ? <div className="installed-list">{updates.map((item) => <div className="installed-row" key={str(item.reference)}><div className="app-glyph"><ArrowDownToLine /></div><div className="installed-info"><b>{str(item.reference)}</b><span>{str(item.current_version)} → {str(item.available_version, 'No update available')}</span></div></div>)}</div> : <EmptyState title="You’re all caught up" detail="We’ll show available updates here." action={<button className="quiet-button" onClick={() => void rpc<Json[]>('CheckUpdates').then(setUpdates).catch((reason: unknown) => setError(String(reason)))}><RefreshCw size={15} /> Check again</button>} />}</section>
            : page === 'health' ? <section><PageHeading eyebrow="LOCAL SERVICE" title="System health" description="A quick check of npack’s configuration and package store." action={<button className="quiet-button" onClick={() => void rpc<Json>('RunDoctor', { user: true }).then(setHealth).catch((reason: unknown) => setError(String(reason)))}><RefreshCw size={15} /> Run check</button>} />{health && <div className="health-card"><div className="health-mark"><ShieldCheck /></div><div><b>{health.healthy === true ? 'Everything looks good' : 'Review these findings'}</b><p>{asList<Json>(health.findings).length} checks from the npack service</p></div></div>}<Findings value={health} /></section>
            : <Settings config={config} onSave={async (relaysText, blossomText) => { try { const saved = await rpc<Json>('SaveConfig', { relays: relaysText.split('\n').map((x) => x.trim()).filter(Boolean), blossom: blossomText.split('\n').map((x) => x.trim()).filter(Boolean) }); setConfig(saved); setNotice('Preferences saved.'); void browse(true, category, query, asList<string>(saved.relays)); } catch (reason) { setError(String(reason)); } }} />}
      </div>
    </main>
    {trustConfirm && selected && <Modal title="Review before installing" close={() => setTrustConfirm(false)}>{selected.trust !== 'trusted' && <div className="trust-note"><ShieldCheck size={20} /><div><b>This publisher isn’t in your trusted list.</b><p>npack verifies the publisher’s signed release and checks the downloaded file’s SHA-256. Trust confirms who published it; it does not guarantee that the software is safe.</p></div></div>}<div className="publisher-card"><span className="publisher-avatar">{displayName(selected).slice(0, 1).toUpperCase()}</span><div><b>{displayName(selected)}</b><small>{selected.npub ?? publisherShort(selected.publisher)}</small></div><span className={`badge ${selected.trust === 'trusted' ? 'trusted' : 'unknown'}`}>{selected.trust === 'trusted' ? 'TRUSTED' : 'UNTRUSTED'}</span></div>{packageCapabilities(selected).map((capability) => <label className="capability-choice" key={capability}><input type="checkbox" checked={serviceAllowed} onChange={(event) => setServiceAllowed(event.target.checked)} /><span>Allow <b>{capability}</b> to register a service for this install. npack will install the service file but won’t enable or start it.</span></label>)}<div className="modal-actions"><button className="quiet-button" onClick={() => setTrustConfirm(false)}>Cancel</button><button className="primary-button" onClick={() => void install(selected, selected.trust !== 'trusted')} disabled={busy || (packageCapabilities(selected).length > 0 && !serviceAllowed)}>{busy ? 'Installing…' : selected.trust === 'trusted' ? 'Install with approval' : 'Trust once and install'}</button></div></Modal>}
    {removeConfirm && <Modal title={`Remove ${displayName(removeConfirm)}?`} close={() => setRemoveConfirm(null)}><p className="modal-copy">This removes version {removeConfirm.version} and its installed files. Other packages that depend on it may stop working.</p><div className="publisher-card"><span className="app-glyph"><Package /></span><div><b>{displayName(removeConfirm)}</b><small>Version {removeConfirm.version}</small></div></div><div className="modal-actions"><button className="quiet-button" onClick={() => setRemoveConfirm(null)}>Keep package</button><button className="danger-button" onClick={() => void remove(removeConfirm)} disabled={busy}><Trash2 size={15} /> Remove package</button></div></Modal>}
    {upload && <Modal title="Unsigned package" close={discardUpload}><div className="trust-note warning-note"><AlertTriangle size={20} /><div><b>This file has no publisher signature.</b><p>The package archive hash and manifest were checked. Its claimed publisher cannot be verified.</p></div></div><div className="publisher-card"><span className="app-glyph"><FileUp /></span><div><b>{str(upload.preview.name)}</b><small>Version {str(upload.preview.version)} · {size(upload.preview.size)}</small></div></div>{typeof upload.preview.blocked_by === 'string' ? <p className="blocked-note">{upload.preview.blocked_by}</p> : null}{asList<string>(upload.preview.required_capabilities).map((capability) => <label className="capability-choice" key={capability}><input type="checkbox" checked={localAllowed.includes(capability)} onChange={(event) => setLocalAllowed(event.target.checked ? [...localAllowed, capability] : localAllowed.filter((item) => item !== capability))} /><span>Allow <b>{capability}</b> for this install</span></label>)}<div className="modal-actions"><button className="quiet-button" onClick={discardUpload} disabled={busy}>Cancel</button><button className="primary-button" onClick={() => void installUpload()} disabled={busy || Boolean(upload.preview.blocked_by) || asList<string>(upload.preview.required_capabilities).some((item) => !localAllowed.includes(item))}>{busy ? 'Installing…' : 'Install unsigned file'}</button></div></Modal>}
    {dropActive && <div className="drop-overlay"><div><Upload size={30} /><b>Drop an .npk file to inspect it</b><span>It won’t install until you review and confirm.</span></div></div>}
  </div>;
}

function NavButton({ active, icon, label, count, onClick }: { active: boolean; icon: React.ReactNode; label: string; count?: number; onClick: () => void }) {
  return <button className={`nav-button ${active ? 'active' : ''}`} onClick={onClick}>{icon}<span>{label}</span>{count !== undefined && <small>{count}</small>}</button>;
}
function pageTitle(page: Page) { return ({ browse: 'Discover', installed: 'Installed', updates: 'Updates', health: 'System health', settings: 'Preferences' })[page]; }
function progressText(progress: OperationProgress): string {
  if (progress.stage === 'completed') return 'Finishing up…';
  const stages: Record<string, string> = { starting: 'Starting…', connecting: 'Connecting to release sources…', resolving: 'Checking package and dependencies…', downloading: 'Downloading and verifying files…', installing: 'Installing package files…', installed: 'Package installed…', updating: 'Checking for updates…' };
  return progress.detail || stages[progress.stage] || 'Working…';
}
function PageHeading({ eyebrow, title, description, action }: { eyebrow: string; title: string; description: string; action?: React.ReactNode }) { return <div className="page-heading"><div><div className="eyebrow">{eyebrow}</div><h1>{title}</h1><p>{description}</p></div>{action}</div>; }
function PackageCard({ row, onClick }: { row: PackageRow; onClick: () => void }) {
  const cats = asList<string>((row.app as Json | undefined)?.categories);
  return <button className="package-card" onClick={onClick}><div className="card-top"><span className={`app-glyph glyph-${Math.abs(row.name.length % 5)}`}><Package /></span><span className={`badge ${row.trust === 'trusted' ? 'trusted' : 'unknown'}`}>{row.trust === 'trusted' ? 'TRUSTED' : 'NEW PUBLISHER'}</span></div><b className="card-title">{displayName(row)}</b><p>{summary(row)}</p><div className="card-bottom"><span>{cats[0] ?? 'Software'}</span><span>v{row.version}</span></div></button>;
}
function PackageDetails({ row, onBack, onInstall, installed }: { row: PackageRow; onBack: () => void; onInstall: () => void; installed: boolean }) {
  const app = row.app ?? {};
  const details = row.details as Json | undefined;
  return <section><button className="back-button" onClick={onBack}><ArrowLeft size={16} /> Back to catalogue</button><div className="detail-hero"><span className="app-glyph large"><Package /></span><div><div className="eyebrow">PACKAGE DETAILS</div><h1>{displayName(row)}</h1><p>{summary(row)}</p></div><button className="primary-button" onClick={onInstall} disabled={installed}>{installed ? <><Check size={16} /> Installed</> : <><ArrowDownToLine size={16} /> Install</>}</button></div><div className="detail-columns"><article><h2>About this package</h2><p className="description">{str(app.description, summary(row))}</p>{typeof app.release_notes === 'string' && <><h2>What’s new</h2><p className="description">{app.release_notes}</p></>}{asList<string>(details?.post_install).length > 0 && <><h2>Install actions</h2><ul className="action-list">{asList<string>(details?.post_install).map((effect) => <li key={effect}>{effect}</li>)}</ul></>}{asList<string>(details?.requires).length > 0 && <><h2>Runtime requirements</h2><ul className="action-list">{asList<string>(details?.requires).map((requirement) => <li key={requirement}>{requirement}</li>)}</ul></>}</article><aside className="facts"><div><span>Publisher</span><b>{row.npub ?? publisherShort(row.publisher)}</b></div><div><span>Trust</span><b>{row.trust === 'trusted' ? 'Trusted by you' : 'Not trusted'}</b></div><div><span>Version</span><b>{row.version}</b></div><div><span>Download</span><b>{size(app.download_size)}</b></div><div><span>License</span><b>{str(app.license, 'Not listed')}</b></div>{typeof app.homepage === 'string' && app.homepage.startsWith('https://') && <a href={app.homepage} target="_blank" rel="noreferrer">Project website ↗</a>}</aside></div></section>;
}
function EmptyState({ title, detail, action }: { title: string; detail: string; action: React.ReactNode }) { return <div className="empty-state"><span className="empty-icon"><Heart size={19} /></span><b>{title}</b><p>{detail}</p>{action}</div>; }
function Findings({ value }: { value: Json | null }) { const findings = asList<Json>(value?.findings); return <div className="findings">{findings.map((item, index) => <div key={`${str(item.title)}-${index}`} className="finding"><span className={`finding-dot ${item.level === 'fail' ? 'fail' : item.level === 'warn' ? 'warn' : ''}`} /><div><b>{str(item.title)}</b>{typeof item.detail === 'string' && <p>{item.detail}</p>}{typeof item.fix === 'string' && <small>Suggested fix: {item.fix}</small>}</div><span className="finding-level">{str(item.level).toUpperCase()}</span></div>)}</div>; }
function Settings({ config, onSave }: { config: Json; onSave: (relays: string, blossom: string) => Promise<void> }) {
  const [relays, setRelays] = useState(asList<string>(config.relays).join('\n'));
  const [servers, setServers] = useState(asList<string>(config.blossom).join('\n'));
  useEffect(() => { setRelays(asList<string>(config.relays).join('\n')); setServers(asList<string>(config.blossom).join('\n')); }, [config]);
  return <section><PageHeading eyebrow="YOUR PREFERENCES" title="Preferences" description="Choose where npack discovers releases and retrieves package files." /><div className="settings-panel"><label>Discovery relays<span>One ws:// or wss:// relay per line.</span><textarea value={relays} onChange={(event) => setRelays(event.target.value)} rows={5} /></label><label>Download servers<span>Blossom servers, one URL per line.</span><textarea value={servers} onChange={(event) => setServers(event.target.value)} rows={4} /></label><div className="security-copy"><ShieldCheck size={18} /><span>Every release signature and artifact hash is checked by npack before installation.</span></div><button className="primary-button" onClick={() => void onSave(relays, servers)}>Save preferences</button></div></section>;
}
function Modal({ title, close, children }: { title: string; close: () => void; children: React.ReactNode }) { return <div className="modal-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) close(); }}><section className="modal" role="dialog" aria-modal="true" aria-label={title}><div className="modal-heading"><h2>{title}</h2><button className="icon-button" onClick={close} aria-label="Close"><X size={18} /></button></div>{children}</section></div>; }
