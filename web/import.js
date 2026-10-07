'use strict';

// Browser-only parsing: keep URL text intact so fragments and signed queries survive.
globalThis.PageCatchImport = (() => {
  const maxFileBytes = 1024 * 1024;
  function csvFields(text) {
    const fields = [];
    let field = '', quoted = false, closed = false;
    for (let i = 0; i < text.length; i++) {
      const char = text[i];
      if (quoted) {
        if (char === '"' && text[i + 1] === '"') { field += '"'; i++; }
        else if (char === '"') { quoted = false; closed = true; }
        else field += char;
      } else if (char === ',' || char === '\r' || char === '\n') {
        fields.push(field); field = ''; closed = false;
        if (char === '\r' && text[i + 1] === '\n') i++;
      } else if (char === '"' && !closed && !field.trim()) {
        field = ''; quoted = true;
      } else if (char === '"' || (closed && char.trim())) {
        throw new Error('Invalid CSV quoting. Export as CSV again or save as TXT.');
      } else if (!closed) field += char;
    }
    if (quoted) throw new Error('Unclosed quote in CSV. Export as CSV again or save as TXT.');
    fields.push(field);
    return fields;
  }
  function cleanUrl(candidate, wrapper) {
    let value = candidate;
    if (wrapper === "'" && value.endsWith("'")) value = value.slice(0, -1);
    if (!/[?#]/.test(value)) value = value.replace(/[.,;!?]+$/, '');
    // Remove sentence punctuation and unmatched Markdown/chat wrappers only.
    while (value) {
      const pairs = { ')': '(', ']': '[', '}': '{' };
      const close = value.at(-1), open = pairs[close];
      if (open === wrapper && open && value.split(close).length > value.split(open).length) {
        value = value.slice(0, -1); continue;
      }
      break;
    }
    if (!/^https?:\/\/[^/?#]/i.test(value) || value.includes('\\')) return null;
    try {
      const parsed = new URL(value);
      if (!['http:', 'https:'].includes(parsed.protocol) || !parsed.hostname || parsed.username || parsed.password) return null;
      return value;
    } catch { return null; }
  }
  function extract(text, format = 'txt') {
    const clean = text.replace(/^\uFEFF/, '');
    const fields = format === 'csv' ? csvFields(clean) : [clean];
    const urls = [], seen = new Set();
    let duplicates = 0;
    for (const field of fields) {
      const candidates = field.matchAll(/https?:\/\/[^\s<>"\u0000-\u001f，。！？；：、（）【】「」『』《》]+/gi);
      for (const candidate of candidates) {
        const value = cleanUrl(candidate[0], field[candidate.index - 1]);
        if (!value) continue;
        if (seen.has(value)) duplicates++;
        else { seen.add(value); urls.push(value); }
      }
    }
    return { urls, duplicates };
  }
  function decode(bytes) {
    const bom = bytes[0] * 256 + bytes[1];
    if (bom === 0xfffe || bom === 0xfeff) {
      return new TextDecoder(bom === 0xfffe ? 'utf-16le' : 'utf-16be', { fatal: true }).decode(bytes);
    }
    try { return new TextDecoder('utf-8', { fatal: true }).decode(bytes); }
    catch { return new TextDecoder('gb18030', { fatal: true }).decode(bytes); }
  }
  async function readFile(file) {
    const extension = file.name.split('.').at(-1).toLowerCase();
    if (!['txt', 'csv'].includes(extension)) throw new Error('Choose a TXT or CSV file.');
    if (file.size > maxFileBytes) throw new Error('File is too large. Choose a file up to 1 MB.');
    let text;
    try { text = decode(new Uint8Array(await file.arrayBuffer())); }
    catch { throw new Error('Cannot read this file. Save it as UTF-8 TXT or CSV and try again.'); }
    const result = extract(text, extension);
    if (!result.urls.length) throw new Error('No valid HTTP/HTTPS URLs found. Check that links start with http:// or https://.');
    return result;
  }
  return Object.freeze({ extract, readFile, maxFileBytes });
})();
