document.getElementById('getTitle').addEventListener('click', () => {
  chrome.tabs.query({active: true, currentWindow: true}, (tabs) => {
    chrome.tabs.sendMessage(tabs[0].id, {action: 'getTitle'}, (response) => {
      if (chrome.runtime.lastError) {
        console.error('錯誤:', chrome.runtime.lastError);
        return;
      }
      const title = response.title;
      document.getElementById('title').textContent = title;
      console.log('獲取到的標題:', title);
      sendTitleToRustApp(title);
    });
  });
});

function sendTitleToRustApp(title) {
  console.log('正在發送標題到 Rust 應用程序...');
  fetch('http://localhost:8000/title', {
    method: 'POST',
    headers: {'Content-Type': 'application/json'},
    body: JSON.stringify({title: title})
  })
  .then(response => response.text())
  .then(data => console.log('Rust 應用程序回應:', data))
  .catch(error => console.error('發送標題時出錯:', error));
}