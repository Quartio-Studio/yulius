import { Ac as defineStd, Ki as noiseTone, Ml as p, aa as unitized, ea as seededPlane, oa as waveletBands, qo as stops, zo as rampOver } from "./std-ClftfWpF.js";
import { i as noiseToneProps, r as noiseSpeedProp, t as noiseColorProps } from "./noiseColor-D_bPhJ4m.js";
const componentDefinition = defineStd({
	name: "WaveletNoise",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Rotating banded wavelets that ripple as they animate",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	props: {
		...noiseColorProps(),
		scale: {
			default: 1.5,
			description: "Pattern scale (higher = larger patterns)",
			ui: {
				type: "range",
				min: -2,
				max: 5,
				step: .1,
				label: "Scale",
				group: "Effect"
			}
		},
		detail: {
			default: 1.24,
			description: "Per-octave frequency ratio — higher adds finer wavelet detail",
			ui: {
				type: "range",
				min: 1.05,
				max: 2,
				step: .01,
				label: "Detail",
				group: "Effect"
			}
		},
		...noiseToneProps(),
		...noiseSpeedProp()
	},
	paint: rampOver(noiseTone(seededPlane(unitized(waveletBands({ detail: p("detail") })), {
		scale: p("scale"),
		seed: p("seed")
	}), {
		contrast: p("contrast"),
		balance: p("balance")
	}), stops(p("colorSpace")))
});
var WaveletNoise_default = componentDefinition;
export { componentDefinition as n, WaveletNoise_default as t };
