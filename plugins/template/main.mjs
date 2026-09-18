// An Agentty plugin. Guide: PLUGIN_GUIDE.md · Types: agentty-plugin.d.ts
import { createPlugin, ui } from './agentty-plugin.mjs';

const plugin = createPlugin();
let clicks = 0;
const name = () => plugin.info?.plugin.name ?? 'Plugin';

function render(context) {
  const pane = context?.pane;
  return plugin.setPanel(
    ui.column([
      ui.text(name(), 'title'),
      ui.text(pane ? `Focused: ${pane.title} · ${pane.cwd}` : 'No terminal is focused.', 'muted'),
      ui.row([
        ui.button('hello', `Say hello (${clicks})`, { icon: 'sparkles', variant: 'primary' }),
        ui.button('explain', 'Explain this folder', { icon: 'bot' }),
      ]),
    ]),
  );
}

async function explain(context) {
  await plugin.injectPrompt({
    text: 'Give me a short tour of this project: what it does, how it is organized and how to run it.',
    title: 'Project tour',
    cwd: context?.pane?.cwd,
    target: 'ask',
  });
}

plugin
  .onPanelOpen(render)
  .onContextChange(render)
  .onEvent('hello', async (_event, context) => {
    clicks += 1;
    await plugin.notify(`Hello from ${name()}!`, 'success');
    await render(context);
  })
  .onEvent('explain', (_event, context) => explain(context))
  .command('{{id}}.explain', ({ context }) => explain(context))
  .start();
