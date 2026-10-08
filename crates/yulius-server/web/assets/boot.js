// Runs before the first paint (a classic, render-blocking script in <head>): applies the stored
// theme so a light or system theme never flashes dark while the app loads. theme.js takes over
// afterwards and keeps the same key and default (web/README.md "Theming and branding").
(function () {
  var choice = 'dark';
  try {
    var stored = JSON.parse(window.localStorage.getItem('yulius.theme'));
    if (stored === 'dark' || stored === 'light' || stored === 'system') choice = stored;
  } catch (error) { /* no storage: the default theme */ }
  var theme = choice === 'system' ? (window.matchMedia('(prefers-color-scheme: light)').matches ? 'light' : 'dark') : choice;
  document.documentElement.setAttribute('data-theme', theme);
  var scheme = document.querySelector('meta[name="color-scheme"]');
  if (scheme) scheme.setAttribute('content', theme);
}());
