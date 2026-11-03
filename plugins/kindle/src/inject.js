// MangaReeda のキー設定に合うキーを横取りし、プラグインに動作の名前を知らせる。
// 合わないキーは Kindle にそのまま渡す。プラグインが送り直したキーは、その1回だけ素通しにする。
(() => {
  if (window.__mangareedaInstalled) return;
  window.__mangareedaInstalled = true;
  const matchers = __MATCHERS__;

  // ライブラリは本を新しいタブで開き、アプリ表示ではそれが別の窓になる。
  // MangaReeda と同じく1つの窓で読むように、リーダーへの移動は同じ窓で行う
  const isReader = (url) => {
    try {
      const u = new URL(url, location.href);
      // ライブラリは /kindle-library/manga-wr/<ASIN> を開き、そこから /manga/<ASIN> へ移る。
      // 文字の本は未確認なので、…-wr/ の形を広く拾う
      return (
        /^read\.amazon\./.test(u.hostname) &&
        (/^\/manga\//.test(u.pathname) || /\/[a-z]+(-[a-z]+)*-wr\//.test(u.pathname) || u.searchParams.has('asin'))
      );
    } catch {
      return false;
    }
  };
  // 単ページ: Cloud Reader に単ページと見開きの切り替えは無いが、窓が縦長だと1ページずつ表示する。
  // body を中央の縦長の枠に絞り（transform で position: fixed の子も枠に収める）、
  // リーダーが読む innerWidth をその枠の幅にする。どちらも窓の大きさから毎回計算する
  const SINGLE_RATIO = 0.7;
  let single = false;
  const realWidth = Object.getOwnPropertyDescriptor(window, 'innerWidth');
  const narrowWidth = () =>
    Math.round(Math.min(document.documentElement.clientWidth, window.innerHeight * SINGLE_RATIO));
  const singleStyle = document.createElement('style');
  singleStyle.textContent = `
    body {
      position: fixed !important; top: 0 !important; height: 100vh !important;
      width: min(100vw, ${SINGLE_RATIO * 100}vh) !important;
      left: calc((100vw - min(100vw, ${SINGLE_RATIO * 100}vh)) / 2) !important;
      transform: translateZ(0) !important; overflow: hidden !important;
    }
    html { background: #000 !important; }`;
  const isReaderPage = () => /^\/manga\//.test(location.pathname);
  window.__mangareedaSetSingle = (on) => {
    single = !!on && isReaderPage();
    if (single) {
      Object.defineProperty(window, 'innerWidth', { configurable: true, get: narrowWidth });
      (document.head || document.documentElement).appendChild(singleStyle);
    } else {
      singleStyle.remove();
      if (realWidth) Object.defineProperty(window, 'innerWidth', realWidth);
      else delete window.innerWidth;
    }
    // リーダーに大きさを測り直させる
    window.dispatchEvent(new Event('resize'));
  };

  const originalOpen = window.open;
  window.open = function (url, ...rest) {
    if (url && isReader(url)) {
      location.assign(url);
      return null;
    }
    return originalOpen.call(this, url, ...rest);
  };
  window.addEventListener(
    'click',
    (e) => {
      const a = e.target && e.target.closest && e.target.closest('a[target]');
      if (a && a.target !== '_self' && isReader(a.href)) {
        e.preventDefault();
        e.stopImmediatePropagation();
        location.assign(a.href);
      }
    },
    true,
  );
  const matches = (m, e) =>
    m.ctrl === (e.ctrlKey || e.metaKey) &&
    m.alt === e.altKey &&
    // 記号のキーは Shift の有無が key に表れるので、key で決まるときは Shift を見ない
    (m.shift === e.shiftKey || (m.codes.length === 0 && m.keys.some((k) => k.length === 1 && !/[a-z0-9 ]/i.test(k)))) &&
    (m.codes.includes(e.code) || m.keys.some((k) => k.toLowerCase() === (e.key || '').toLowerCase()));
  window.addEventListener(
    'keydown',
    (e) => {
      if (window.__mangareedaPassKey && window.__mangareedaPassKey === e.key) {
        window.__mangareedaPassKey = null;
        return;
      }
      // 文字の入力欄（ライブラリの検索など）では横取りしない
      const t = e.target;
      if (t && (t.isContentEditable || /^(INPUT|TEXTAREA|SELECT)$/.test(t.tagName))) return;
      const m = matchers.find((m) => matches(m, e));
      if (!m) return;
      e.preventDefault();
      e.stopImmediatePropagation();
      if (typeof window.__mangareedaAction === 'function') {
        window.__mangareedaAction(m.action);
      }
    },
    true,
  );
})();
