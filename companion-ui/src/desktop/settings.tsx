// 主题必须最先执行：theme.ts 在模块求值时就给 <html> 加上 dark class，之后
// tailwind.css 才会注入，避免 SettingsForm 挂载前闪一帧浅色。
import { watchTheme } from './theme';
import { SettingsForm } from './components/SettingsForm';
import { installStandaloneWindowChrome } from './host.js';
import { createRoot } from 'react-dom/client';
import '@/styles/tailwind.css';

// Must run before React mounts so the drag strip stays the first child of <body>.
installStandaloneWindowChrome();

/**
 * 设置窗与主窗口同源，主题偏好由主应用写进 localStorage。这里订阅变化：
 * 用户切到深色后，已经打开的悬浮窗设置窗要跟着变，不能等下次打开。
 */
const stopThemeWatch = watchTheme();
window.addEventListener('pagehide', () => stopThemeWatch(), {once: true});

const container = document.getElementById('root');
if (container) createRoot(container).render(<SettingsForm />);
