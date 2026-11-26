import { ICONS, type IconShape } from '../components/icons'

const line = (body: string): IconShape => ({ body })
const loop = '<path d="M4 10V7a3 3 0 0 1 3-3h13M17 1.5L20 4l-3 2.5M20 14v3a3 3 0 0 1-3 3H4M7 17.5L4 20l3 2.5"/>'
const file = '<path d="M14 3H5a1 1 0 0 0-1 1v16a1 1 0 0 0 1 1h14a1 1 0 0 0 1-1V9zM14 3v6h6"/>'
const cookie = '<path d="M20.5 12.5A8.5 8.5 0 1 1 11.5 3a4 4 0 0 0 5 5 4 4 0 0 0 4 4.5z"/><circle cx="8" cy="10" r=".8"/><circle cx="12" cy="16" r=".8"/><circle cx="7" cy="15" r=".8"/>'
// Окно браузера — сплошная полоса сверху, вкладка — полоса со ступенькой: не путаются с папкой.
const browser = '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M3 8.5h18"/>'
const tab = '<rect x="3" y="4" width="18" height="16" rx="2"/><path d="M11 4v5h10"/>'
const waves = '<path d="M5.5 5.5a9 9 0 0 0 0 13M18.5 5.5a9 9 0 0 1 0 13"/>'

/** Иконки операций: единая геометрия 24×24 и контур 1.7px для палитры, узлов и настроек. */
export const OP_ICONS: Record<string, IconShape> = {
  start: ICONS.play,
  'control.if': line('<path d="M12 2.5v3M12 5.5l3.5 3.5-3.5 3.5-3.5-3.5zM8.5 9H5v11M15.5 9H19v11M2.5 17.5 5 20l2.5-2.5M16.5 17.5 19 20l2.5-2.5"/>'),
  'control.for_times': line(loop + '<path d="M10 9l-1 6M14 9l-1 6M8 11h8M8 14h8"/>'),
  'control.for_data': line(loop + '<path d="M8 9h8M8 12h8M8 15h5"/>'),
  'control.while': line(loop + '<path d="M12 8l4 4-4 4-4-4z"/>'),
  'control.break': line('<path d="M15 10a6 6 0 1 0-6 6h12M18 13l3 3-3 3"/>'),
  'control.call': line('<rect x="3" y="3" width="7" height="6" rx="1.5"/><rect x="14" y="15" width="7" height="6" rx="1.5"/><path d="M6.5 9v4h11v2M14.5 12l3 3 3-3"/>'),
  'control.quit_browser': line(browser + '<path d="M9.5 12l5 5M14.5 12l-5 5"/>'),
  'control.group': ICONS.groupNodes,
  'control.for_elements': line(loop + '<rect x="8" y="8" width="5" height="5" rx="1"/><path d="M13 11h3v5h-5v-3"/>'),
  'control.new_browser': line(browser + '<path d="M12 11.5v6M9 14.5h6"/>'),

  'proc.vars': line('<path d="M7 4H6a2 2 0 0 0-2 2v4l-2 2 2 2v4a2 2 0 0 0 2 2h1M17 4h1a2 2 0 0 1 2 2v4l2 2-2 2v4a2 2 0 0 1-2 2h-1M9 9l6 6M15 9l-6 6"/>'),
  'proc.regex': line('<circle cx="5" cy="17" r="1"/><path d="M15 4v12M10 7l10 6M20 7l-10 6"/>'),
  'proc.to_json': line('<path d="M8 4H6a2 2 0 0 0-2 2v4l-2 2 2 2v4a2 2 0 0 0 2 2h2M16 4h2a2 2 0 0 1 2 2v4l2 2-2 2v4a2 2 0 0 1-2 2h-2"/><circle cx="12" cy="9" r=".7"/><circle cx="12" cy="15" r=".7"/>'),
  'proc.field': line('<path d="M8 4H7a2 2 0 0 0-2 2v4l-2 2 2 2v4a2 2 0 0 0 2 2h1M10 12h11M17.5 8.5 21 12l-3.5 3.5"/>'),
  'proc.random': line('<rect x="3" y="3" width="18" height="18" rx="3"/><circle cx="8" cy="8" r=".9"/><circle cx="12" cy="12" r=".9"/><circle cx="16" cy="16" r=".9"/>'),

  'web.new_tab': line(tab + '<path d="M12 12v5M9.5 14.5h5"/>'),
  'web.close_tab': line(tab + '<path d="M10 12.5l4 4M14 12.5l-4 4"/>'),
  'web.close_other_tabs': line('<rect x="2" y="6" width="9" height="12" rx="1.5"/><rect x="13" y="6" width="9" height="12" rx="1.5"/><path d="M2 9.5h9M13 9.5h9M15.5 12l4 4M19.5 12l-4 4"/>'),
  'web.switch_tab': line(tab + '<path d="M7 12.5h10M14.5 10l2.5 2.5-2.5 2.5M17 17H7M9.5 14.5 7 17l2.5 2.5"/>'),
  'web.goto': ICONS.proxies,
  'web.reload': ICONS.refresh,
  'web.back': line('<path d="M19 12H5M11 6l-6 6 6 6"/>'),
  'web.js': line('<path d="M8 6l-6 6 6 6M16 6l6 6-6 6M14 3l-4 18"/>'),
  'web.screenshot': line('<path d="M8 6l2-3h4l2 3h4a2 2 0 0 1 2 2v11a2 2 0 0 1-2 2H4a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2z"/><circle cx="12" cy="13" r="4"/>'),
  'web.hover': line('<path d="M5 3l14 10-7 1-3 7zM15 4h5M19 3v4"/>'),
  'web.select': line('<rect x="3" y="5" width="18" height="14" rx="2"/><path d="M6 9h6M6 14h4M15 11l2 2 2-2"/>'),
  'web.focus': line('<path d="M8 3H3v5M16 3h5v5M3 16v5h5M21 16v5h-5"/><circle cx="12" cy="12" r="3"/>'),
  'web.click': line('<path d="M8 8l12 8-6 1-3 5zM8 2v2M2 8h2M3 3l2 2M13 3l-2 2M3 13l2-2"/>'),
  'web.input': line('<path d="M13 7h7a2 2 0 0 1 2 2v6a2 2 0 0 1-2 2h-7M5 7H4a2 2 0 0 0-2 2v6a2 2 0 0 0 2 2h1M9 5v14M6.5 5h5M6.5 19h5"/>'),
  'web.scroll': line('<path d="M12 3v18M7 8l5-5 5 5M7 16l5 5 5-5"/>'),
  'web.upload': line(file + '<path d="M12 18v-6M9 15l3-3 3 3"/>'),

  'key.press': ICONS.keyboard,
  'key.combo': line('<path d="M9 7V5a2 2 0 1 0-2 2h10a2 2 0 1 0-2-2v14a2 2 0 1 0 2-2H7a2 2 0 1 0 2 2V7z"/>'),
  'wait.time': ICONS.clock,
  'wait.element': line('<rect x="2" y="3" width="15" height="12" rx="2"/><path d="M2 7h15"/><circle cx="16" cy="16" r="6"/><path d="M16 12v4l2 1"/>'),
  'wait.request': line('<path d="M3 7h15M15 4l3 3-3 3M3 11h5M3 15h3"/><circle cx="15" cy="16" r="6"/><path d="M15 12v4l2 1"/>'),

  'data.element': line('<path d="M2 22l1-1h3l9-9M3 21v-3l9-9M15 6l3.4-3.4a2.1 2.1 0 1 1 3 3L18 9l.4.4a2.1 2.1 0 1 1-3 3l-3.8-3.8a2.1 2.1 0 1 1 3-3l.4.4z"/>'),
  'data.focused': line('<path d="M8 3H3v5M16 3h5v5M3 16v5h5M14 17h8M18 13l4 4-4 4"/><circle cx="11" cy="11" r="3"/>'),
  'data.url': line('<path d="M10 7l2-2a5 5 0 0 1 7 7l-2 2M14 17l-2 2a5 5 0 0 1-7-7l2-2M8 16l8-8"/>'),
  'data.clipboard': ICONS.paste,
  'data.save_txt': line(file + '<path d="M7 8h3M7 12h3M13 12v6M10 15l3 3 3-3"/>'),
  'data.save_excel': line('<path d="M11 21H3V3h14v7M3 8h14M3 14h7M8 8v13M16 12v9M12 17l4 4 4-4"/>'),
  'data.download': ICONS.download,
  'data.import_txt': line('<path d="M8 3h7l5 5v12a1 1 0 0 1-1 1H8M15 3v5h5M2 12h11M9 8l4 4-4 4M11 18h5"/>'),
  'data.import_excel': line('<path d="M9 3h12v18H9M9 8h12M9 14h12M15 8v13M2 12h9M7 8l4 4-4 4"/>'),
  'data.totp': line('<path d="M13 18H4a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2v3M6.5 10.4v3.2M5.11 11.2l2.77 1.6M5.11 12.8l2.77-1.6M11.5 10.4v3.2M10.11 11.2l2.77 1.6M10.11 12.8l2.77-1.6"/><circle cx="18" cy="17" r="4.5"/><path d="M18 15v2l1.3 1"/>'),
  'data.email': line('<rect x="2" y="4" width="20" height="16" rx="2"/><path d="M2 6l10 7L22 6"/>'),
  'data.listen_request': line(waves + '<path d="M12 17V7M8.5 10.5 12 7l3.5 3.5"/>'),
  'data.listen_response': line(waves + '<path d="M12 7v10M8.5 13.5 12 17l3.5-3.5"/>'),
  'data.listen_stop': line(waves + '<rect x="9" y="9" width="6" height="6" rx="1"/>'),
  'data.cookies_get': line(cookie),
  'data.cookies_clear': line('<path d="M20 11A4 4 0 0 1 16 7a4 4 0 0 1-4-4 9 9 0 1 0-1 18M15 15l6 6M21 15l-6 6"/><circle cx="8" cy="10" r=".8"/><circle cx="8" cy="16" r=".8"/>'),

  'ext.openai': line('<path d="M12 2l3 7 7 3-7 3-3 7-3-7-7-3 7-3zM20 2v4M18 4h4"/>'),
  'ext.google_sheet': line('<rect x="3" y="3" width="18" height="18" rx="2"/><path d="M3 9h18M3 15h18M9 3v18M15 9v12"/>'),
  'ext.2captcha': line('<path d="M12 2l8 3v6c0 5-4 8-8 11-4-3-8-6-8-11V5zM8 12l3 3 5-6"/>'),
  'profile.remark': line('<path d="M20 10V4H4v16h6M8 8h8M8 12h3M13 17l6-6 3 3-6 6-4 1z"/>'),
  'profile.tags': line('<path d="M3 3h8l11 11-8 8L3 11z"/><circle cx="7.5" cy="7.5" r="1.2"/>'),
}

export const opIcon = (id: string): IconShape => OP_ICONS[id] ?? ICONS.automation
