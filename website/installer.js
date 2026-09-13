const status = document.querySelector('#version');
fetch('manifest.json', { cache: 'no-cache' })
  .then(response => {
    if (!response.ok) throw new Error('Release unavailable');
    return response.json();
  })
  .then(manifest => {
    status.textContent = `Release ${manifest.version}`;
    document.querySelector('#installer').hidden = false;
  })
  .catch(() => {
    status.textContent = 'The release could not be loaded. Please reload this page later.';
  });
