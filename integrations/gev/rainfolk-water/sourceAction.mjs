// This UI opens only explicit, source-supplied official USGS record links.
// No fallback guesses, outbound requests, HTML interpolation or click handlers.
const OFFICIAL_ORIGIN = 'https://waterdata.usgs.gov';
export function safeOfficialSourceUrl(raw) {
  if (typeof raw !== 'string' || !raw || /[\u0000-\u0020\u007f\\]/.test(raw)) return null;
  try {
    const url = new URL(raw);
    if (raw === 'https://www.hydroshare.org/resource/8b02895f02c14dd1a749bcc5584a5c55/') return raw;
    if (url.origin !== OFFICIAL_ORIGIN || url.username || url.password) return null;
    return url.href;
  } catch { return null; }
}
export function createSourceAction(container) {
  const label = container.querySelector('#source-record');
  const link = container.querySelector('#original-source');
  const unavailable = container.querySelector('#source-unavailable');
  return function render(record) {
    const href = safeOfficialSourceUrl(record?.sourceUrl);
    container.hidden = !record;
    label.textContent = record ? `Selected record: ${typeof record.id === 'string' ? record.id : 'Unknown identifier'}. ` : '';
    link.removeAttribute('href');
    link.hidden = !href;
    unavailable.hidden = !record || Boolean(href);
    if (href) {
      link.setAttribute('href', href);
      link.setAttribute('target', '_blank');
      link.setAttribute('rel', 'noopener noreferrer');
      link.setAttribute('referrerpolicy', 'no-referrer');
    }
  };
}
