document.getElementById('getTitleAndDescription').addEventListener('click', () => {
  const loader = document.getElementById('loader');
  const titleElement = document.getElementById('title');
  const descriptionElement = document.getElementById('description');

  loader.style.display = 'inline-block';
  titleElement.textContent = '';
  descriptionElement.textContent = '';

  chrome.tabs.query({active: true, currentWindow: true}, (tabs) => {
    chrome.tabs.sendMessage(tabs[0].id, {action: 'getTitleAndDescription'}, (response) => {
      loader.style.display = 'none';
      if (chrome.runtime.lastError) {
        console.error('error:', chrome.runtime.lastError);
        titleElement.textContent = 'error.title';
        descriptionElement.textContent = 'error.description';
        return;
      }
      const { title, description } = response;
      titleElement.textContent = title;
      descriptionElement.textContent = description;
      console.log('get title:', title);
      console.log('get description:', description);
      sendDataToRustApp(title, description);
    });
  });
});

function sendDataToRustApp(title, description) {
  console.log('正在發送標題和描述到 Rust 應用程序...');
  fetch('http://localhost:8000/data', {
    method: 'POST',
    headers: {'Content-Type': 'application/json'},
    body: JSON.stringify({title: title, description: description})
  })
  .then(response => response.text())
  .then(data => console.log('Rust 應用程序回應:', data))
  .catch(error => console.error('發送數據時出錯:', error));
}