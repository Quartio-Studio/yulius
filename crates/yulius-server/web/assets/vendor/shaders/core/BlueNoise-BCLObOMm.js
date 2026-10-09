import { Ac as defineStd, Ki as noiseTone, Ml as p, Pi as blueSpeckle, Yi as pixelGrid, qo as stops, zo as rampOver } from "./std-ClftfWpF.js";
import { i as noiseToneProps, t as noiseColorProps } from "./noiseColor-D_bPhJ4m.js";
const componentDefinition = defineStd({
	name: "BlueNoise",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "High-frequency blue noise — even, grainy speckle ideal for dithering",
	acceptsUVContext: true,
	props: {
		...noiseColorProps(),
		grain: {
			default: 1,
			description: "Grain size in pixels (1 = per-pixel, higher = chunkier speckle)",
			ui: {
				type: "range",
				min: 1,
				max: 32,
				step: 1,
				label: "Grain Size",
				group: "Effect"
			}
		},
		...noiseToneProps()
	},
	paint: rampOver(noiseTone(pixelGrid(blueSpeckle(), {
		grain: p("grain"),
		seed: p("seed")
	}), {
		contrast: p("contrast"),
		balance: p("balance")
	}), stops(p("colorSpace")))
});
var BlueNoise_default = componentDefinition;
export { componentDefinition as n, BlueNoise_default as t };
