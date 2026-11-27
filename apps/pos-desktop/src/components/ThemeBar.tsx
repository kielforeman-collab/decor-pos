import { useState } from 'react'
import type { ThemeDef, ThemeId } from '../themes'

interface ThemeBarProps {
  themes: ThemeDef[]
  themeId: ThemeId
  themeNote: string
  liveMonitors: boolean
  onSetTheme: (id: ThemeId) => void
  onToggleLiveMonitors: (on: boolean) => void
  onAddCustomTheme: (t: { name: string; bg: string; ink: string; accent: string }) => void
  onToast: (msg: string) => void
}

/**
 * Compact theme switcher — lives inside the top bar, not its own row.
 * A native <select> keeps it to one control (~32px tall) that never
 * grows the bar, and it still switches every theme. The custom-theme
 * lab opens as a popover under the bar instead of a permanent row.
 * The Live monitors toggle is TOS-only (the monitors are TOS-only).
 */
export default function ThemeBar({
  themes,
  themeId,
  themeNote,
  liveMonitors,
  onSetTheme,
  onToggleLiveMonitors,
  onAddCustomTheme,
  onToast,
}: ThemeBarProps) {
  const [labOpen, setLabOpen] = useState(false)
  const [name, setName] = useState('Night Counter')
  const [bg, setBg] = useState('#101826')
  const [ink, setInk] = useState('#e8eef4')
  const [accent, setAccent] = useState('#00d9ff')

  return (
    <div className="theme-switch-compact">
      <label className="theme-select-label" htmlFor="theme-select">Theme</label>
      <select
        id="theme-select"
        className="theme-select"
        value={themeId}
        title={themeNote}
        aria-label="Theme"
        onChange={(e) => onSetTheme(e.target.value as ThemeId)}
      >
        {themes.map((t) => (
          <option key={t.id} value={t.id}>
            {t.label}
          </option>
        ))}
      </select>
      <button
        type="button"
        className="theme-add-btn"
        aria-expanded={labOpen}
        title="Add a custom theme"
        onClick={() => setLabOpen((v) => !v)}
      >
        + Custom
      </button>
      {themeId === 'tos' && (
        <button
          type="button"
          className="live-toggle"
          role="switch"
          aria-checked={liveMonitors}
          onClick={() => onToggleLiveMonitors(!liveMonitors)}
        >
          <span className="live-dot" aria-hidden="true" />
          Live {liveMonitors ? 'On' : 'Off'}
        </button>
      )}

      {labOpen && (
        <form
          className="custom-lab"
          onSubmit={(e) => {
            e.preventDefault()
            if (!name.trim()) return
            onAddCustomTheme({ name: name.trim(), bg, ink, accent })
            onToast(`Custom theme “${name.trim()}” added & applied.`)
            setLabOpen(false)
          }}
        >
          <strong>Custom theme lab</strong>
          <label>
            Name
            <input value={name} onChange={(e) => setName(e.target.value)} />
          </label>
          <label>
            Background
            <input type="color" value={bg} onChange={(e) => setBg(e.target.value)} />
          </label>
          <label>
            Ink
            <input type="color" value={ink} onChange={(e) => setInk(e.target.value)} />
          </label>
          <label>
            Accent
            <input type="color" value={accent} onChange={(e) => setAccent(e.target.value)} />
          </label>
          <button type="submit">Add theme &amp; apply</button>
        </form>
      )}
    </div>
  )
}
