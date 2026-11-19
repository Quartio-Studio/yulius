// The target and the harness share the worker's engine so startup can observe failed requests.
import { surfaceOf, web } from '@e2e-dev/web';

export const chromium = web({ viewport: { width: 1440, height: 900 } });
export const surface = surfaceOf(chromium)!;
