function getYouTubeTitle() {
    const titleElement = document.querySelector('h1.ytd-video-primary-info-renderer');
    return titleElement ? titleElement.textContent.trim() : '無法獲取標題';
  }
  
  chrome.runtime.onMessage.addListener((request, sender, sendResponse) => {
    if (request.action === 'getTitle') {
      sendResponse({ title: getYouTubeTitle() });
    }
  });