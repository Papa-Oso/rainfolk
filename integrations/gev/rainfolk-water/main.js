// Water-only shell adapted from the existing standalone preview and tested
// groundwater component harness. No app bootstrap, providers or producer code.
import * as Cesium from 'cesium';
import { createUsGroundwaterLayer } from '../../src/layers/usGroundwater/index.js';
import { historicalWellDetails } from '../../src/layers/usGroundwater/historicalPack.mjs';
import { createSpatialWellSource } from '../../src/layers/usGroundwater/spatialSource.js';
import * as overlays from '../../src/overlays/worldOverlay.js';
import { initTrackedReadout } from '../../src/data/trackedReadout.js';
import { clearSelectedEntityContextForLayer } from '../../src/data/contextStore.js';
import '../../src/ui/styles/foundation.css';
import './map.css';
import { createSourceAction } from './sourceAction.mjs';
import { installMapLifecycle } from './pageLifecycle.mjs';

Cesium.Ion.defaultAccessToken = '';
const viewer = new Cesium.Viewer('map', {
  baseLayer: false,
  terrainProvider: new Cesium.EllipsoidTerrainProvider(),
  baseLayerPicker: false,
  geocoder: false,
  animation: false,
  timeline: false,
  homeButton: false,
  sceneModePicker: false,
  navigationHelpButton: false,
  fullscreenButton: false,
  infoBox: false,
  selectionIndicator: false,
  skyBox: false,
  skyAtmosphere: false,
});
viewer.scene.globe.baseColor = Cesium.Color.fromCssColorString('#123247');
viewer.scene.backgroundColor = Cesium.Color.fromCssColorString('#07131e');
viewer.scene.globe.enableLighting = false;
overlays.initWorldOverlay(viewer);
initTrackedReadout(viewer);
const status = document.querySelector('#status');
const renderSourceAction = createSourceAction(document.querySelector('#selected-source'));
const removeSelectionListener = viewer.selectedEntityChanged.addEventListener(entity => {
  if (!entity) renderSourceAction(null);
});
let enabled = true;
let basemapError = false;
const layer = createUsGroundwaterLayer({
  source: createSpatialWellSource({base:'/well-index-usgwd/', label:'USGWD historical catalog', onCatalog:counts=>{document.querySelector('#retained').textContent=counts.catalogRecords?.toLocaleString() ?? '-';document.querySelector('#unmapped').textContent=counts.unmappedRecords?.toLocaleString() ?? '-';}}),
  sourceLabel:'USGWD historical catalog; location accuracy unverified',
  describeWell:historicalWellDetails,
  overlays,
  onSelect: renderSourceAction,
  onStatus(stats) {
    document.querySelector('#catalog').textContent =
      stats.catalogCount == null ? '—' : stats.catalogCount.toLocaleString();
    document.querySelector('#represented').textContent =
      stats.representedLocationCount.toLocaleString();
    status.textContent = stats.error
      ? `Update unavailable: ${stats.error}. Any previous view remains a saved snapshot.`
      : !enabled
        ? 'Wells hidden.'
        : stats.loading
          ? 'Updating the loaded view…'
          : basemapError
            ? 'Basemap unavailable; source positions remain on the globe.'
            : 'Historical source records; uncertain locations · coverage incomplete';
  },
});
const home = () =>
  viewer.camera.setView({
    destination: Cesium.Cartesian3.fromDegrees(-98, 38, 6000000),
    orientation: { heading: 0, pitch: -Math.PI / 2, roll: 0 },
  });
home();
layer.init(viewer);
try {
  viewer.imageryLayers.addImageryProvider(
    await Cesium.TileMapServiceImageryProvider.fromUrl(
      Cesium.buildModuleUrl('Assets/Textures/NaturalEarthII/'),
    ),
  );
} catch {
  basemapError = true;
}
await layer.enable();
document.querySelector('#home').addEventListener('click', home);
document.querySelector('#toggle').addEventListener('click', async (event) => {
  enabled = !enabled;
  event.currentTarget.textContent = enabled ? 'Hide wells' : 'Show wells';
  event.currentTarget.setAttribute('aria-pressed', String(enabled));
  if (enabled) await layer.enable();
  else layer.disable();
});
function dismiss() {
  viewer.selectedEntity = undefined;
  clearSelectedEntityContextForLayer('us-groundwater');
  viewer.scene.requestRender();
}
document.querySelector('#dismiss').addEventListener('click', dismiss);
document.addEventListener('keydown', (event) => {
  if (event.key === 'Escape') dismiss();
});
installMapLifecycle(window, {
  dispose() {
    removeSelectionListener();
    layer.destroy();
    overlays.destroyWorldOverlay();
    viewer.destroy();
  },
  resume() {
    viewer.resize();
    viewer.scene.requestRender();
  },
});
// Read-only acceptance seam, matching the earlier independent preview.
window.__rainfolkMap = {
  getStats: () => layer.getStats(),
  getOwnedResources: () => ({ dataSources: viewer.dataSources.length }),
  getMapState: () => ({
    tilesLoaded: viewer.scene.globe.tilesLoaded,
    imagery: viewer.imageryLayers.length,
  }),
};
