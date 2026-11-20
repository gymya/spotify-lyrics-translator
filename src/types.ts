export interface Track { id: string; title: string; artist: string; album: string; durationMs: number }
export interface Playback { track: Track | null; positionMs: number; isPlaying: boolean; status: 'playing' | 'paused' | 'stopped' | 'unavailable'; observedAtMonotonicMs: number; message: string | null }
export interface Line { startTimeMs: number; original: string; translated?: string }
export interface Candidate { id: number; title: string; artist: string; album: string; durationMs: number; kind: 'synced' | 'unsynced' | 'instrumental' }
export type Lyrics = { kind: 'synced'; lines: Line[] } | { kind: 'unsynced'; text: string } | { kind: 'instrumental' } | { kind: 'not_found' } | { kind: 'candidates'; candidates: Candidate[] } | { kind: 'error'; message: string }
export interface Settings { translationProvider: 'google' | 'azure'; azureRegion: string; opacity: number; fontSize: number; originalColor: string; translatedColor: string; subtitleBackground: boolean; offsetMs: number; clickThrough: boolean; alwaysOnTop: boolean; overlayEnabled: boolean; x: number | null; y: number | null; panelX: number | null; panelY: number | null }
export interface Snapshot { playback: Playback; lyrics: Lyrics | null; line: Line | null; loading: boolean; translating: boolean; translationMessage: string | null; settings: Settings; editing: boolean }
