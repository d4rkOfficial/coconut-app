// @name        护眼滤镜
// @version     1.0.0
// @description 给工具箱加一层暖色滤镜，并在右下角放一个小浮层
// @permissions dom, ui
// @mount       overlay

// 浮层 id 由引擎分配（1、2、3…），插件自己记住就行
let overlayId = 0;

const CSS = `
  .eye-care-card {
    position: absolute;
    right: 20px;
    bottom: 20px;
    padding: 10px 14px;
    border-radius: 12px;
    background: rgba(28, 24, 18, 0.86);
    color: #f3e6cf;
    font-size: 12px;
    line-height: 1.6;
    box-shadow: 0 12px 30px rgba(0, 0, 0, 0.4);
    pointer-events: auto; /* 浮层默认不拦鼠标，要交互的元素自己打开 */
  }
`;

async function onHello() {
  Coconut.ui.injectCss('eye-care', 'html { filter: sepia(0.22) saturate(1.05) !important; }');
  overlayId = Coconut.ui.overlay({
    html: '<div class="eye-care-card">护眼滤镜已开启（sepia 22%）</div>',
    css: CSS,
  });
  Coconut.log('护眼滤镜已挂到工具箱页面，浮层 id =', overlayId);

  // 3 秒后演示一次隐藏 / 显示
  await Coconut.sleep(3000);
  Coconut.ui.hide(overlayId);
  await Coconut.sleep(600);
  Coconut.ui.show(overlayId);
}

async function onStop() {
  Coconut.ui.injectCss('eye-care', '');
  if (overlayId) Coconut.ui.remove(overlayId);
  overlayId = 0;
}
