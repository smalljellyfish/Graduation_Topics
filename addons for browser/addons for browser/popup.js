function cleanDescription(description) {
  if (description === null) return null;
  
  // 移除特殊 Unicode 字符
  description = description.replace(/[\u{a0}\u{200B}-\u{200D}\u{FEFF}]/gu, ' ');
  
  // 將多個空格替換為單個空格
  description = description.replace(/\s+/g, ' ');
  
  // 移除行首行尾的空白字符
  description = description.trim();
  
  // 將換行符統一為 \n
  description = description.replace(/\r\n|\r|\n/g, '\n');
  
  // 過濾掉包含 HTTPS 連結的行，但保留其他內容
  description = description.split('\n')
    .map(line => line.replace(/https?:\/\/\S+/g, '').trim())
    .filter(line => line.length > 0)
    .join('\n');
  
  // 再次移除可能出現的多餘空行
  description = description.replace(/\n+/g, '\n').trim();
  
  return description.length > 0 ? description : null;
}

document.getElementById('getTitleAndDescription').addEventListener('click', () => {
  const loader = document.getElementById('loader');
  const titleElement = document.getElementById('title');
  const descriptionElement = document.getElementById('description');

  loader.style.display = 'inline-block';
  titleElement.textContent = '';
  descriptionElement.textContent = '';

  chrome.tabs.query({active: true, currentWindow: true}, (tabs) => {
    chrome.tabs.sendMessage(tabs[0].id, {action: 'getTitleAndDescription'}, (response) => {
      if (chrome.runtime.lastError) {
        console.error('錯誤:', chrome.runtime.lastError);
        loader.style.display = 'none';
        return;
      }
      const { title, description } = response;
      titleElement.textContent = title;
      if (description !== null) {
        const cleanedDescription = cleanDescription(description);
        descriptionElement.textContent = cleanedDescription || '無有效描述';
        console.log('獲取到的標題:', title);
        console.log('清理後的描述:', cleanedDescription);
        sendDataToRustApp(title, cleanedDescription);
      } else {
        descriptionElement.textContent = '無描述';
        console.log('獲取到的標題:', title);
        console.log('描述: 無');
        sendDataToRustApp(title, null);
      }
      
      loader.style.display = 'none';
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
  .then(response => response.json())
  .then(data => {
    console.log('Rust 應用程序回應:', data);
    const resultElement = document.getElementById('result');
    resultElement.textContent = data.result;
    if (data.cached) {
      resultElement.textContent += ' (來自快取)';
    }
  })
  .catch(error => {
    console.error('發送數據時出錯:', error);
    document.getElementById('result').textContent = '發送數據時出錯，請查看控制台';
  });
}