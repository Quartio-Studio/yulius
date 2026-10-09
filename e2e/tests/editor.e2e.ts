import { test } from '@e2e-dev/web';
import { expect } from 'e2e';
import { failOnPageErrors, mock } from './support.ts';

failOnPageErrors();

test('a file from the tree opens in an editor tab', async ({ app, screen }) => {
  await app.open(mock('/s/studio/t/12'));
  await screen.getByRole('button', /^Show tools/).click();
  await screen.getByRole('button', 'Editor').click();

  const tree = screen.getByRole('tree', 'Files');
  await tree.getByRole('treeitem', 'README.md').click();

  await expect(screen.getByRole('tab', /^README\.md/)).toHaveAttribute('aria-selected', 'true');
  await expect(screen.getByRole('textbox', 'Editor content')).toBeAttached();

  const folder = tree.getByRole('treeitem', /src$/);
  await expect(folder).toHaveAttribute('aria-expanded', 'false');
  await folder.click();
  await expect(folder).toHaveAttribute('aria-expanded', 'true');
});
