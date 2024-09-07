function getYouTubeTitle() {
  const titleElement = document.querySelector('h1.ytd-video-primary-info-renderer');
  return titleElement ? titleElement.textContent.trim() : '無法獲取標題';
}

function getYouTubeDescription() {
  return new Promise((resolve) => {
    const maxAttempts = 5;
    let attempts = 0;

    function tryGetDescription() {
      attempts++;
      
      // 嘗試展開描述
      const showMoreButton = document.querySelector('#description-inline-expander #expand');
      if (showMoreButton) {
        showMoreButton.click();
        setTimeout(extractDescription, 1000);
      } else {
        extractDescription();
      }
    }

    function extractDescription() {
      const descriptionElement = document.querySelector('yt-attributed-string.ytd-text-inline-expander');
      if (descriptionElement) {
        // 遞迴函數來提取純文本
        function extractText(element) {
          let text = '';
          for (let node of element.childNodes) {
            if (node.nodeType === Node.TEXT_NODE) {
              text += node.textContent;
            } else if (node.nodeType === Node.ELEMENT_NODE) {
              if (node.tagName.toLowerCase() === 'a') {
                text += node.href + ' ';
              }
              text += extractText(node);
            }
          }
          return text;
        }

        const description = extractText(descriptionElement).trim();
        console.log('Raw description:', description); // 用於調試
        if (description) {
          resolve(description);
        } else if (attempts < maxAttempts) {
          setTimeout(tryGetDescription, 1000);
        } else {
          resolve('無法獲取描述');
        }
      } else if (attempts < maxAttempts) {
        setTimeout(tryGetDescription, 1000);
      } else {
        resolve('無法獲取描述');
      }
    }

    tryGetDescription();
  });
}

chrome.runtime.onMessage.addListener((request, sender, sendResponse) => {
  if (request.action === 'getTitleAndDescription') {
    Promise.all([getYouTubeTitle(), getYouTubeDescription()]).then(([title, description]) => {
      console.log('Title:', title); // 用於調試
      console.log('Description:', description); // 用於調試
      sendResponse({
        title: title,
        description: description
      });
    });
    return true; // 表示我們將異步發送回應
  }
});