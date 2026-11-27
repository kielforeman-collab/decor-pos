import { useEffect, useRef, useState } from 'react'
import { searchProducts } from '../api'
import { fmtMoney, cad, type ProductSearchResult } from '../types'
import ThemeBar from './ThemeBar'
import type { ThemeDef, ThemeId } from '../themes'

/** Inline-SVG nebula monitors (TOS only — CSS hides them on other
 * themes). No raster files, no external images. Each is 4:3. */
function MonitorSvg({ variant, label }: { variant: 0 | 1 | 2 | 3; label: string }) {
  const id = `mon${variant}`
  const palettes = [
    { a: '#7b2ff7', b: '#2b0a4d', glow: '#b678ff', accent: '#e0b3ff' },
    { a: '#00c2a8', b: '#032b33', glow: '#5ef2dd', accent: '#b8fff2' },
    { a: '#ff7b1c', b: '#331400', glow: '#ffb020', accent: '#ffd9a0' },
    { a: '#3d9948', b: '#062b12', glow: '#7be495', accent: '#d2ffd9' },
  ][variant]
  return (
    <div className={`monitor mon-${variant}`} role="img" aria-label={label}>
      <svg viewBox="0 0 96 72" aria-hidden="true">
        <defs>
          <radialGradient id={`${id}-neb`} cx="50%" cy="45%" r="65%">
            <stop offset="0%" stopColor={palettes.a} />
            <stop offset="55%" stopColor={palettes.b} />
            <stop offset="100%" stopColor="#050505" />
          </radialGradient>
        </defs>
        <rect width="96" height="72" fill="#050505" />
        <rect width="96" height="72" fill={`url(#${id}-neb)`} />
        <g className="neb-drift" fill={palettes.glow} opacity="0.55">
          <ellipse cx="34" cy="30" rx="16" ry="9" />
          <ellipse cx="62" cy="44" rx="13" ry="7" />
        </g>
        <g className="stars" fill="#fff">
          <circle cx="12" cy="10" r="1" />
          <circle cx="28" cy="18" r="0.8" />
          <circle cx="48" cy="8" r="1" />
          <circle cx="70" cy="16" r="0.8" />
          <circle cx="86" cy="10" r="1" />
          <circle cx="20" cy="52" r="0.8" />
          <circle cx="58" cy="60" r="1" />
          <circle cx="82" cy="54" r="0.8" />
          <circle cx="40" cy="62" r="0.8" />
        </g>
        {variant === 3 && (
          <g>
            <ellipse cx="48" cy="36" rx="20" ry="5" fill="none" stroke={palettes.accent} strokeWidth="1.4" transform="rotate(-18 48 36)" />
            <circle cx="48" cy="36" r="9" fill={palettes.a} stroke={palettes.accent} strokeWidth="1" />
          </g>
        )}
        {variant === 1 && (
          <g fill={palettes.accent} opacity="0.9">
            <circle cx="30" cy="26" r="2.2" />
            <circle cx="64" cy="42" r="1.8" />
            <circle cx="52" cy="20" r="1.4" />
          </g>
        )}
      </svg>
    </div>
  )
}

export function BridgeMonitors() {
  return (
    <div className="monitors" aria-label="Bridge monitors">
      <MonitorSvg variant={0} label="Purple nebula" />
      <MonitorSvg variant={1} label="Cyan nebula" />
      <MonitorSvg variant={2} label="Orange nebula" />
      <MonitorSvg variant={3} label="Green nebula, ringed planet" />
    </div>
  )
}

interface TopBarProps {
  storeName: string
  tillOpen: boolean
  openingFloat: string
  clerkName: string
  onOmniAdd: (p: ProductSearchResult) => void
  onToast: (msg: string) => void
  themes: ThemeDef[]
  themeId: ThemeId
  themeNote: string
  liveMonitors: boolean
  onSetTheme: (id: ThemeId) => void
  onToggleLiveMonitors: (on: boolean) => void
  onAddCustomTheme: (t: { name: string; bg: string; ink: string; accent: string }) => void
}

export default function TopBar({
  storeName,
  tillOpen,
  openingFloat,
  clerkName,
  onOmniAdd,
  onToast,
  themes,
  themeId,
  themeNote,
  liveMonitors,
  onSetTheme,
  onToggleLiveMonitors,
  onAddCustomTheme,
}: TopBarProps) {
  const [now, setNow] = useState(() => new Date())
  const [query, setQuery] = useState('')
  const [results, setResults] = useState<ProductSearchResult[]>([])
  const [open, setOpen] = useState(false)
  const [highlight, setHighlight] = useState(0)
  const boxRef = useRef<HTMLDivElement>(null)

  useEffect(() => {
    const t = window.setInterval(() => setNow(new Date()), 20000)
    return () => window.clearInterval(t)
  }, [])

  useEffect(() => {
    if (query.trim().length < 2) {
      setResults([])
      setOpen(false)
      return
    }
    const h = window.setTimeout(() => {
      void searchProducts({ q: query.trim(), limit: 5 }).then((r) => {
        setResults(r)
        setOpen(r.length > 0)
        setHighlight(0)
      })
    }, 200)
    return () => window.clearTimeout(h)
  }, [query])

  useEffect(() => {
    const onDoc = (e: MouseEvent) => {
      if (boxRef.current && !boxRef.current.contains(e.target as Node)) setOpen(false)
    }
    document.addEventListener('mousedown', onDoc)
    return () => document.removeEventListener('mousedown', onDoc)
  }, [])

  const clock = now.toLocaleTimeString('en-CA', { hour: '2-digit', minute: '2-digit' })

  const pick = (p: ProductSearchResult) => {
    onOmniAdd(p)
    setQuery('')
    setResults([])
    setOpen(false)
  }

  return (
    <header className="topbar">
      <button
        type="button"
        className="menu-btn"
        aria-label="Toggle navigation"
        onClick={() => document.documentElement.classList.toggle('nav-open')}
      >
        ☰
      </button>
      <div className="brand">
        <span className="brand-name">
          <svg className="delta-shield" viewBox="0 0 24 24" aria-hidden="true">
            <path d="M12 1 L22 8 L18 23 L12 18 L6 23 L2 8 Z" fill="currentColor" />
          </svg>
          DECOR POS
        </span>
        <span className="brand-chip">UI MOCKUP</span>
      </div>
      <div className="till-text">
        Register 1 · Main counter · {tillOpen ? `Drawer open · ${fmtMoney(cad(openingFloat || '0.0000'))} float` : 'Drawer closed'}
      </div>
      <BridgeMonitors />
      <div className="topbar-spacer" />
      <ThemeBar
        themes={themes}
        themeId={themeId}
        themeNote={themeNote}
        liveMonitors={liveMonitors}
        onSetTheme={onSetTheme}
        onToggleLiveMonitors={onToggleLiveMonitors}
        onAddCustomTheme={onAddCustomTheme}
        onToast={onToast}
      />
      <div className="omni" ref={boxRef}>
        <svg className="omni-icon" viewBox="0 0 24 24" aria-hidden="true">
          <circle cx="10.5" cy="10.5" r="6.5" fill="none" stroke="currentColor" strokeWidth="2" />
          <line x1="15.5" y1="15.5" x2="21" y2="21" stroke="currentColor" strokeWidth="2" />
        </svg>
        <input
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === 'ArrowDown' && results.length > 0) {
              e.preventDefault()
              setHighlight((h) => Math.min(h + 1, results.length - 1))
            } else if (e.key === 'ArrowUp' && results.length > 0) {
              e.preventDefault()
              setHighlight((h) => Math.max(h - 1, 0))
            } else if (e.key === 'Enter') {
              if (results.length > 0) {
                e.preventDefault()
                pick(results[highlight] ?? results[0])
              }
            } else if (e.key === 'Escape') {
              setQuery('')
              setOpen(false)
            }
          }}
          placeholder="Search products, customers, invoices…"
          aria-label="Search products, customers, invoices"
        />
        {open && (
          <div className="omni-results" role="listbox">
            {results.map((r, i) => (
              <button
                key={r.product_id}
                type="button"
                role="option"
                aria-selected={i === highlight}
                className={i === highlight ? 'hl' : ''}
                onClick={() => pick(r)}
              >
                <span className="omni-sku">{r.sku}</span>
                <span className="omni-name">{r.name}</span>
                <span className="omni-price">List {fmtMoney(cad(r.list_price))}</span>
              </button>
            ))}
          </div>
        )}
      </div>
      <div className="top-meta">
        <span className="clock">{clock}</span>
        <span className="clerk">Clerk: {clerkName}</span>
        <span className="store-name" title={storeName}>{storeName}</span>
      </div>
    </header>
  )
}
