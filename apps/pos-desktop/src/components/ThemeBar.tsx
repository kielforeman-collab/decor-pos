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

function stardate(now: Date): string {
  const start = new Date(now.getFullYear(), 0, 0)
  const day = Math.floor((now.getTime() - start.getTime()) / 86400000)
  return `${now.getFullYear()}.${String(day).padStart(3, '0')}`
}

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
    <div className="theme-col">
      <div className="themebar">
        <div className="theme-switch" role="group" aria-label="Theme">
          <span className="theme-label">Theme</span>
          {themes.map((t) => (
            <button
              key={t.id}
              type="button"
              className="theme-btn"
              aria-pressed={themeId === t.id}
              onClick={() => onSetTheme(t.id)}
            >
              {t.shortLabel}
            </button>
          ))}
          <button
            type="button"
            className="theme-btn theme-add"
            aria-pressed={labOpen}
            onClick={() => setLabOpen((v) => !v)}
          >
            + Custom
          </button>
        </div>
        <span id="themeLayoutNote" className="theme-note">{themeNote}</span>
      </div>

      <div className="captains-strip" aria-label="Captain's chair strip">
        <span className="cap-log">Captain&rsquo;s Log</span>
        <span className="cap-stardate">Stardate {stardate(new Date())}</span>
        <span className="cap-div cap-command">Command</span>
        <span className="cap-div cap-science">Science</span>
        <span className="cap-div cap-ops">Operations</span>
        <span className="cap-lamps" aria-hidden="true">
          <i className="lamp lamp-gold" />
          <i className="lamp lamp-blue" />
          <i className="lamp lamp-red" />
          <i className="lamp lamp-green" />
        </span>
        <button
          type="button"
          className="live-toggle"
          role="switch"
          aria-checked={liveMonitors}
          onClick={() => onToggleLiveMonitors(!liveMonitors)}
        >
          <span className="live-dot" aria-hidden="true" />
          Live monitors {liveMonitors ? 'On' : 'Off'}
        </button>
      </div>

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
