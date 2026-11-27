// Theme registry — DecorPOSThemes-style: themes are token + layout
// plugins only. No screen code may branch on a theme name; screens
// read CSS custom properties. `setTheme` sets <html data-theme> +
// data-layout, aria-pressed on the theme buttons, and the theme
// button label. Theme choice, custom themes and the live-monitor
// state persist in localStorage (session-only in the mockup).

import { useCallback, useEffect, useState } from 'react'

export type BuiltinThemeId = 'expanse' | 'mono' | 'dark' | 'lcars' | 'tos'
export type ThemeId = BuiltinThemeId | string // string = custom:<id>

export interface ThemeDef {
  id: ThemeId
  label: string
  shortLabel: string
  layout: 'default' | 'lcars' | 'tos'
  note: string
  /** Layout plugin values applied inline on <html>. */
  touchMin: string
  pill: string
  elbow: string
  rail: string
  blockGap: string
}

export const BUILTIN_THEMES: ThemeDef[] = [
  {
    id: 'expanse',
    label: 'Expanse Ops',
    shortLabel: 'Expanse',
    layout: 'default',
    note: 'Expanse Ops — deep-space navy, ice-cyan instruments, MCRN-orange actions.',
    touchMin: '40px',
    pill: '6px',
    elbow: '0px',
    rail: '3px',
    blockGap: '6px',
  },
  {
    id: 'mono',
    label: 'Document · B/W',
    shortLabel: 'B/W',
    layout: 'default',
    note: 'Document · B/W — the wild base geometry: thick rules, monochrome receipt style.',
    touchMin: '40px',
    pill: '6px',
    elbow: '0px',
    rail: '3px',
    blockGap: '6px',
  },
  {
    id: 'dark',
    label: 'Dark · W/B',
    shortLabel: 'Dark',
    layout: 'default',
    note: 'Dark · W/B — white-on-black, same Document geometry.',
    touchMin: '40px',
    pill: '6px',
    elbow: '0px',
    rail: '3px',
    blockGap: '6px',
  },
  {
    id: 'lcars',
    label: 'LCARS · TNG',
    shortLabel: 'LCARS',
    layout: 'lcars',
    note: 'LCARS · TNG — pill blocks, elbow frames, 6px gaps everywhere, blocks never touch.',
    touchMin: '60px',
    pill: '999px',
    elbow: '52px',
    rail: '26px',
    blockGap: '6px',
  },
  {
    id: 'tos',
    label: 'TOS · Classic',
    shortLabel: 'TOS',
    layout: 'tos',
    note: 'TOS · Classic — chunky bridge controls, 56px touch, gold/blue/red division colours.',
    touchMin: '56px',
    pill: '6px',
    elbow: '0px',
    rail: '8px',
    blockGap: '5px',
  },
]

export const DEFAULT_THEME_ID: ThemeId = 'tos'

export interface CustomTheme {
  id: string
  name: string
  bg: string
  ink: string
  accent: string
}

const THEME_KEY = 'decor-pos-theme'
const CUSTOM_THEMES_KEY = 'decor-pos-custom-themes'
const LIVE_MONITORS_KEY = 'decor-pos-live-monitors'

function loadCustomThemes(): CustomTheme[] {
  try {
    const raw = window.localStorage.getItem(CUSTOM_THEMES_KEY)
    if (!raw) return []
    const parsed = JSON.parse(raw) as CustomTheme[]
    return Array.isArray(parsed) ? parsed.filter((t) => t?.id && t?.name) : []
  } catch {
    return []
  }
}

function themeDefFor(id: ThemeId, customs: CustomTheme[]): ThemeDef {
  const builtin = BUILTIN_THEMES.find((t) => t.id === id)
  if (builtin) return builtin
  const custom = customs.find((c) => `custom:${c.id}` === id)
  if (custom) {
    return {
      id,
      label: custom.name,
      shortLabel: custom.name,
      layout: 'default',
      note: `${custom.name} — custom theme (bg ${custom.bg}, ink ${custom.ink}, accent ${custom.accent}).`,
      touchMin: '40px',
      pill: '6px',
      elbow: '0px',
      rail: '3px',
      blockGap: '6px',
    }
  }
  return BUILTIN_THEMES.find((t) => t.id === DEFAULT_THEME_ID)!
}

function applyThemeToDom(def: ThemeDef, custom: CustomTheme | undefined, liveMonitors: boolean): void {
  const root = document.documentElement
  root.setAttribute('data-theme', def.layout === 'default' && def.id.startsWith('custom:') ? 'custom' : def.id)
  root.setAttribute('data-layout', def.layout)
  root.style.setProperty('--touch-min', def.touchMin)
  root.style.setProperty('--pill', def.pill)
  root.style.setProperty('--elbow', def.elbow)
  root.style.setProperty('--rail', def.rail)
  root.style.setProperty('--block-gap', def.blockGap)
  if (custom) {
    root.style.setProperty('--custom-bg', custom.bg)
    root.style.setProperty('--custom-ink', custom.ink)
    root.style.setProperty('--custom-accent', custom.accent)
  } else {
    root.style.removeProperty('--custom-bg')
    root.style.removeProperty('--custom-ink')
    root.style.removeProperty('--custom-accent')
  }
  root.classList.toggle('mon-live', liveMonitors)
}

export function useTheme() {
  const [customThemes, setCustomThemes] = useState<CustomTheme[]>(() => loadCustomThemes())
  const [themeId, setThemeIdState] = useState<ThemeId>(() => {
    try {
      return window.localStorage.getItem(THEME_KEY) || DEFAULT_THEME_ID
    } catch {
      return DEFAULT_THEME_ID
    }
  })
  const [liveMonitors, setLiveMonitorsState] = useState<boolean>(() => {
    try {
      return window.localStorage.getItem(LIVE_MONITORS_KEY) !== 'off'
    } catch {
      return true
    }
  })

  const def = themeDefFor(themeId, customThemes)
  const custom = themeId.startsWith('custom:')
    ? customThemes.find((c) => `custom:${c.id}` === themeId)
    : undefined

  useEffect(() => {
    applyThemeToDom(def, custom, liveMonitors)
    try {
      window.localStorage.setItem(THEME_KEY, themeId)
    } catch {
      // storage unavailable — theme still applies for the session
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [themeId, liveMonitors, customThemes])

  const setTheme = useCallback((id: ThemeId) => {
    setThemeIdState(id)
  }, [])

  const cycleTheme = useCallback(() => {
    const all: ThemeId[] = [...BUILTIN_THEMES.map((t) => t.id), ...customThemes.map((c) => `custom:${c.id}`)]
    const idx = all.indexOf(themeId)
    setThemeIdState(all[(idx + 1) % all.length] ?? DEFAULT_THEME_ID)
  }, [themeId, customThemes])

  const addCustomTheme = useCallback((t: Omit<CustomTheme, 'id'>) => {
    const id = `c${Date.now().toString(36)}`
    const next = [...customThemes, { ...t, id }]
    setCustomThemes(next)
    try {
      window.localStorage.setItem(CUSTOM_THEMES_KEY, JSON.stringify(next))
    } catch {
      // ignore
    }
    setThemeIdState(`custom:${id}`)
    return `custom:${id}`
  }, [customThemes])

  const setLiveMonitors = useCallback((on: boolean) => {
    setLiveMonitorsState(on)
    try {
      window.localStorage.setItem(LIVE_MONITORS_KEY, on ? 'on' : 'off')
    } catch {
      // ignore
    }
  }, [])

  return {
    themeId,
    themeDef: def,
    customThemes,
    allThemes: [
      ...BUILTIN_THEMES,
      ...customThemes.map((c): ThemeDef => themeDefFor(`custom:${c.id}`, customThemes)),
    ],
    setTheme,
    cycleTheme,
    addCustomTheme,
    liveMonitors,
    setLiveMonitors,
  }
}
