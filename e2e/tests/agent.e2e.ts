import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

// A goal in plain English, checked with an ordinary assertion. Once it passes with a model, the
// runner records the agent's actions under replays/ and later runs replay them without a model.
const noModel = process.env.ANTHROPIC_API_KEY ? false : 'agent steps need a model: set ANTHROPIC_API_KEY';

test('an agent opens a file in the editor from a plain-English goal', { skip: noModel }, async ({ app, agent, screen }) => {
  await app.open(mock('/s/studio/t/12'));

  await agent.act('open the README.md file of this task in the editor');

  await expect(screen.getByRole('tab', /^README\.md/)).toHaveAttribute('aria-selected', 'true');
});
