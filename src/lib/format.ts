import type { GatewayView, ProxyKind } from '../api/types'

export function osMajor(version: string): string {
  return version.split('.')[0] ?? version
}

/** «145.0.7632.109.2» → «145»; без версии или с мусором — `null`. */
export function engineMajor(version: string | null | undefined): string | null {
  const major = version?.split('.')[0]
  return major && /^\d+$/.test(major) ? major : null
}

/** «macOS 15 · Chrome 145»: версию браузера даёт сборка движка. */
export function fingerprintTitle(osVersion: string, brand: string, engineVersion: string | null): string {
  const os = osVersion ? `macOS ${osMajor(osVersion)}` : 'macOS'
  const major = engineMajor(engineVersion)
  return major ? `${os} · ${brand} ${major}` : `${os} · ${brand}`
}

const KIND_LABEL: Record<ProxyKind, string> = { http: 'HTTP', https: 'HTTPS', socks5: 'SOCKS5' }

export function kindLabel(kind: ProxyKind): string {
  return KIND_LABEL[kind]
}

/** Страна и IP выхода, если известны с последнего запуска, иначе адрес прокси. */
export function gatewayTitle(gateway: GatewayView): string {
  return gateway.ip && gateway.country ? `${gateway.country} · ${gateway.ip}` : `${gateway.host}:${gateway.port}`
}

/** «ANGLE (Apple, ANGLE Metal Renderer: Apple M2, Unspecified Version)» → «Apple M2». */
export function gpuName(renderer: string): string {
  const match = /^ANGLE \([^,]+, (.+), [^,]+\)$/.exec(renderer)
  if (!match) return renderer
  return match[1].replace(/^ANGLE Metal Renderer: /, '').replace(/ OpenGL Engine$/, '')
}

/** Дата для имени файла: `ГГГГ-ММ-ДД` по местному времени, не по UTC. */
export function localDate(date: Date): string {
  const two = (n: number) => String(n).padStart(2, '0')
  return `${date.getFullYear()}-${two(date.getMonth() + 1)}-${two(date.getDate())}`
}

const pad2 = (n: number) => String(n).padStart(2, '0')
/** Время строки журнала запуска: ЧЧ:ММ:СС по местному времени. */
export function clock(ms: number): string {
  const d = new Date(ms)
  return `${pad2(d.getHours())}:${pad2(d.getMinutes())}:${pad2(d.getSeconds())}`
}
