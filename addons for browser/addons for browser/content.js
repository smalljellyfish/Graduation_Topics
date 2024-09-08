function getYouTubeTitle() {
  const titleElement = document.querySelector('h1.ytd-video-primary-info-renderer');
  return titleElement ? titleElement.textContent.trim() : '無法獲取標題';
}

function getYouTubeDescription() {
  return new Promise((resolve) => {
    const maxAttempts = 3;
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
        const description = descriptionElement.textContent.trim();
        if (description) {
          resolve(description);
        } else {
          resolve(null); // 描述為空時返回 null
        }
      } else if (attempts < maxAttempts) {
        setTimeout(tryGetDescription, 1000);
      } else {
        resolve(null); // 無法獲取描述時返回 null
      }
    }

    tryGetDescription();
  });
}

chrome.runtime.onMessage.addListener((request, sender, sendResponse) => {
  if (request.action === 'getTitleAndDescription') {
    Promise.all([getYouTubeTitle(), getYouTubeDescription()]).then(([title, description]) => {
      console.log('Title:', title);
      console.log('Description:', description);
      sendResponse({
        title: title,
        description: description // 可能為 null
      });
    });
    return true; // 表示我們將異步發送回應
  }
});


