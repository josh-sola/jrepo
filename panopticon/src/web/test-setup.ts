// bun has no global test preload here, so each DOM test imports this. Every
// test file shares one process, so a second registration throws; swallow
// only that error.
import { GlobalRegistrator } from '@happy-dom/global-registrator';

try {
  GlobalRegistrator.register();
} catch (error) {
  const alreadyRegistered =
    error instanceof Error && error.message.includes('already');
  if (!alreadyRegistered) throw error;
}
