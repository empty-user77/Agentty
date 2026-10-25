// Agentty's markdown preview: VS Code's renderer, set up as its markdown preview sets it up
// (extensions/markdown-language-features/src/markdownEngine.ts) — markdown-it with raw HTML,
// linkify without fuzzy links, highlight.js for fenced code, ids on headings (GitHub style), front
// matter left out — then sanitized with DOMPurify, since nothing a document holds may run here.
(() => {
	'use strict';

	const normalizeHighlightLang = (lang) => {
		switch ((lang || '').toLowerCase()) {
			case 'shell': return 'sh';
			case 'py3': return 'python';
			case 'tsx':
			case 'typescriptreact': return 'jsx';
			case 'json5':
			case 'jsonc': return 'json';
			case 'c#':
			case 'csharp': return 'cs';
			default: return lang;
		}
	};

	const md = window.markdownit({
		html: true,
		breaks: false,
		linkify: true,
		typographer: false,
		highlight: (str, lang) => {
			lang = normalizeHighlightLang(lang);
			if (lang && window.hljs.getLanguage(lang)) {
				try {
					return window.hljs.highlight(str, { language: lang, ignoreIllegals: true }).value;
				} catch (e) { }
			}
			return md.utils.escapeHtml(str);
		},
	});
	md.linkify.set({ fuzzyLink: false });

	// data:image links are allowed, like in VS Code.
	const validateLink = md.validateLink;
	md.validateLink = (link) => validateLink(link) || /^data:image\/.*?;/.test(link);

	// Heading ids the way GitHub (and VS Code) make them, so `#section` links work.
	const plainText = (token) => {
		if (token.children) return token.children.map(plainText).join('');
		return ['text', 'emoji', 'code_inline'].includes(token.type) ? token.content : '';
	};
	const slug = (heading) => heading.trim().toLowerCase().replace(/[^\p{L}\p{M}\p{N}\p{Pc}\- ]/gu, '').replace(/\s/g, '-');
	const headingOpen = md.renderer.rules.heading_open;
	md.renderer.rules.heading_open = (tokens, idx, options, env, self) => {
		let value = slug(plainText(tokens[idx + 1]));
		const seen = env.slugs.get(value);
		if (seen !== undefined) {
			env.slugs.set(value, seen + 1);
			value = `${value}-${seen + 1}`;
		} else {
			env.slugs.set(value, 0);
		}
		tokens[idx].attrSet('id', value);
		return headingOpen ? headingOpen(tokens, idx, options, env, self) : self.renderToken(tokens, idx, options);
	};

	// Each block says which line of the text it starts on, as in VS Code (`data-line`), so the
	// page can follow the text's scrolling.
	md.core.ruler.push('agentty_lines', (state) => {
		for (const token of state.tokens) {
			if (token.map && token.nesting !== -1 && token.type !== 'inline') {
				token.attrSet('data-line', String(token.map[0] + (state.env.offset || 0)));
			}
		}
	});

	// VS Code hides YAML front matter in the preview by default.
	const frontMatter = (text) => (text.match(/^---\r?\n[\s\S]*?\r?\n---[ \t]*(\r?\n|$)/) || [''])[0];

	// The page has no address of its own and reads no files: a local image is handed over by
	// Agentty (`agenttyImages`), and a link goes to Agentty (`agenttyTakeLinks`), which opens web
	// pages in a browser and files in its editor.
	const isLocal = (src) => src && !/^[a-z][a-z0-9+.-]*:/i.test(src) && !src.startsWith('//') && !src.startsWith('#');
	const images = new Map();
	const links = [];

	window.agenttyRender = (text) => {
		const front = frontMatter(text);
		const offset = front ? front.split('\n').length - (front.endsWith('\n') ? 1 : 0) : 0;
		const html = md.render(text.slice(front.length), { slugs: new Map(), offset });
		const target = document.getElementById('agentty-markdown');
		const scroll = document.scrollingElement.scrollTop;
		target.innerHTML = window.DOMPurify.sanitize(html, { ADD_ATTR: ['target'] });
		const wanted = [];
		for (const image of target.querySelectorAll('img[src]')) {
			const src = image.getAttribute('src');
			if (!isLocal(src)) continue;
			image.dataset.src = src;
			if (images.has(src)) {
				image.src = images.get(src);
			} else {
				image.removeAttribute('src');
				wanted.push(src);
			}
		}
		document.scrollingElement.scrollTop = scroll;
		return JSON.stringify([...new Set(wanted)]);
	};

	// Scrolls so that `line` of the text (fractions too) is at the top: between the blocks
	// around it, in proportion.
	window.agenttyScrollTo = (value) => {
		const line = parseFloat(value) || 0;
		const top = (element) => element.getBoundingClientRect().top + window.scrollY;
		let before = null;
		let after = null;
		for (const element of document.querySelectorAll('[data-line]')) {
			const at = Number(element.dataset.line);
			if (at <= line) {
				if (!before || at >= before.at) before = { element, at };
			} else if (!after || at < after.at) {
				after = { element, at };
				break;
			}
		}
		if (!before) {
			document.scrollingElement.scrollTop = 0;
			return 'ok';
		}
		let y = top(before.element);
		if (after && after.at > before.at) {
			y += (top(after.element) - y) * Math.min(1, (line - before.at) / (after.at - before.at));
		}
		document.scrollingElement.scrollTop = Math.max(0, y - 8);
		return 'ok';
	};

	window.agenttyImages = (json) => {
		for (const [src, data] of Object.entries(JSON.parse(json))) {
			images.set(src, data);
			for (const image of document.querySelectorAll('img[data-src]')) {
				if (image.dataset.src === src) image.src = data;
			}
		}
		return 'ok';
	};

	window.agenttyTakeLinks = () => JSON.stringify(links.splice(0));

	// `#section` scrolls here; any other link goes to Agentty.
	document.addEventListener('click', (event) => {
		const link = event.target.closest && event.target.closest('a[href]');
		if (!link) return;
		event.preventDefault();
		const href = link.getAttribute('href');
		if (href.startsWith('#')) {
			const id = decodeURIComponent(href.slice(1));
			const heading = document.getElementById(id) || document.getElementById(id.toLowerCase());
			if (heading) heading.scrollIntoView();
			return;
		}
		if (links.length < 16) links.push(href);
	}, true);
})();
