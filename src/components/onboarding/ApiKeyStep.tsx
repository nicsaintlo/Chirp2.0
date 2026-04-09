import { useState, useEffect, useRef } from 'react'
import { ExternalLink, CheckCircle2, Loader2 } from 'lucide-react'
import { open } from '@tauri-apps/plugin-shell'
import { listen } from '@tauri-apps/api/event'
import { useAppStore } from '../../stores/appStore'
import { useTauri } from '../../hooks/useTauri'
import { Button } from '../shared/Button'

interface ApiKeyStepProps {
  onNext: () => void
}

export function ApiKeyStep({ onNext }: ApiKeyStepProps) {
  const store = useAppStore()
  const tauri = useTauri()
  const [loading, setLoading] = useState(false)
  const [waiting, setWaiting] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [succeeded, setSucceeded] = useState(false)
  const unlistenRef = useRef<(() => void)[]>([])

  useEffect(() => {
    return () => unlistenRef.current.forEach((fn) => fn())
  }, [])

  const handleSignIn = async () => {
    setLoading(true)
    setError(null)

    try {
      const unlisten1 = await listen<{ token: string }>('google-auth-complete', (event) => {
        store.updateSettings({ googleToken: event.payload.token, aiBackend: 'google' })
        setSucceeded(true)
        setWaiting(false)
        setLoading(false)
        setTimeout(() => onNext(), 800)
      })
      const unlisten2 = await listen<{ error: string }>('google-auth-error', (event) => {
        setError(event.payload.error)
        setWaiting(false)
        setLoading(false)
      })
      unlistenRef.current = [unlisten1, unlisten2]

      const authUrl = await tauri.startOauthLogin()
      await open(authUrl)
      setWaiting(true)
      setLoading(false)
    } catch (err) {
      setError(String(err))
      setLoading(false)
    }
  }

  return (
    <div className="flex flex-col animate-fade-in">
      <h1 className="font-display font-extrabold text-2xl text-chirp-stone-900">
        Connect to Google AI
      </h1>
      <p className="mt-1 font-body text-sm text-chirp-stone-500">
        Chirp uses Gemini to transcribe your voice and Gemma to clean up text.
        Sign in with your Google account to get started.
      </p>

      <div className="mt-8">
        {succeeded ? (
          <div className="flex items-center gap-3 text-green-600 font-body text-sm">
            <CheckCircle2 size={20} />
            <span className="font-semibold">Connected! Continuing…</span>
          </div>
        ) : waiting ? (
          <div className="flex flex-col gap-4">
            <div className="flex items-center gap-3 text-chirp-stone-500 font-body text-sm">
              <Loader2 size={18} className="animate-spin text-chirp-amber-500" />
              Waiting for Google sign-in…
            </div>
            {error && (
              <p className="text-sm text-red-500 font-body">{error}</p>
            )}
            <div className="flex gap-3">
              <Button size="onboarding" onClick={handleSignIn}>
                <ExternalLink size={15} className="mr-2" />
                Re-open browser
              </Button>
              <button
                type="button"
                onClick={onNext}
                className="font-body text-sm text-chirp-stone-400 hover:text-chirp-stone-600 transition-colors"
              >
                Skip for now
              </button>
            </div>
          </div>
        ) : (
          <div className="flex flex-col gap-3">
            {error && (
              <div className="rounded-lg bg-red-50 border border-red-200 px-3 py-2 text-sm text-red-700 font-body">
                {error}
              </div>
            )}
            <Button
              size="onboarding"
              className="w-full text-base flex items-center justify-center gap-2"
              onClick={handleSignIn}
              disabled={loading}
            >
              {loading
                ? <><Loader2 size={16} className="animate-spin" /> Opening browser…</>
                : <><ExternalLink size={16} /> Sign in with Google</>
              }
            </Button>
            <button
              type="button"
              onClick={onNext}
              className="font-body text-xs text-chirp-stone-300 hover:text-chirp-stone-400 transition-colors self-center"
            >
              Skip for now
            </button>
          </div>
        )}
      </div>
    </div>
  )
}
