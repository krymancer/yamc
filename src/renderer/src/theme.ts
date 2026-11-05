import { createContext, useContext, useEffect, useState } from 'react'
import { getTheme, type CatAccent, type Theme, type ThemeSetting } from '@shared/themes'

export const ThemeContext = createContext<Theme>(getTheme('zinc-dark', 'mauve', true))
export const useThemeTokens = () => useContext(ThemeContext)

/** Resolve the theme setting (following the OS for "system") and push its tokens into CSS variables. */
export function useTheme(setting: ThemeSetting | undefined, accent: CatAccent | undefined): Theme {
  const mq = window.matchMedia('(prefers-color-scheme: dark)')
  const [systemDark, setSystemDark] = useState(mq.matches)
  useEffect(() => {
    const on = () => setSystemDark(mq.matches)
    mq.addEventListener('change', on)
    return () => mq.removeEventListener('change', on)
  }, [])
  const theme = getTheme(setting ?? 'zinc-dark', accent ?? 'mauve', systemDark)
  useEffect(() => {
    const root = document.documentElement
    for (const [k, v] of Object.entries(theme.vars)) root.style.setProperty(`--${k}`, v)
    root.style.colorScheme = theme.dark ? 'dark' : 'light'
    root.dataset.theme = theme.id
  }, [theme.id, theme.vars.accent])
  return theme
}

export const accountColor = (theme: Theme, index: number) => theme.accountColors[index % theme.accountColors.length]
