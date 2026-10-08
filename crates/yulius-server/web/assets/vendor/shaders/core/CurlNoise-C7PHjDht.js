import { Ac as defineStd, Ki as noiseTone, Ml as p, ea as seededPlane, qo as stops, zi as curlSpeed, zo as rampOver } from "./std-ClftfWpF.js";
import { i as noiseToneProps, r as noiseSpeedProp, t as noiseColorProps } from "./noiseColor-D_bPhJ4m.js";
const componentDefinition = defineStd({
	name: "CurlNoise",
	role: "generator",
	boundingBoxDeclaration: {
		aspectRatio: null,
		supportsResizeFit: true
	},
	category: "Textures",
	description: "Swirling divergence-free flow field that drifts over time",
	acceptsUVContext: true,
	animatedTime: { speed: "speed" },
	props: {
		...noiseColorProps(),
		scale: {
			default: 2,
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
		...noiseToneProps(),
		...noiseSpeedProp()
	},
	paint: rampOver(noiseTone(seededPlane(curlSpeed({ rate: .2 }), {
		scale: p("scale"),
		seed: p("seed")
	}), {
		contrast: p("contrast"),
		balance: p("balance")
	}), stops(p("colorSpace")))
});
var CurlNoise_default = componentDefinition;
export { componentDefinition as n, CurlNoise_default as t };
